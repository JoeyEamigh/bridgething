use std::{
  io::{BufRead, BufReader, Write},
  net::Shutdown,
  os::unix::net::UnixStream,
  path::PathBuf,
  sync::{
    Arc, Mutex,
    mpsc::{Receiver, SendError, Sender, channel},
  },
  thread,
  time::Duration,
};

use bridgething_companion::backend::{AudioBackend, EarconSink, SpeakSink};

use crate::backends::utterance::Utterances;

const CLIENT_NAME: &str = "bridgething:desktop:main";
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const BEGIN: &str = "701 ";
const END: &str = "702 ";
const CANCELED: &str = "703 ";

enum Command {
  Speak { text: String, voice: Option<String> },
  Cancel,
}

pub struct SpeechDispatcher {
  speaking: Arc<Utterances>,
  engine: Mutex<Option<Sender<Command>>>,
}

impl SpeechDispatcher {
  pub fn new() -> Self {
    Self {
      speaking: Arc::new(Utterances::default()),
      engine: Mutex::new(None),
    }
  }

  fn send(&self, command: Command) {
    let mut held = self.engine.lock().unwrap();
    let command = match held.as_ref() {
      Some(engine) => match engine.send(command) {
        Ok(()) => return,
        Err(SendError(command)) => command,
      },
      None => command,
    };

    let (tx, rx) = channel();
    let speaking = Arc::clone(&self.speaking);
    match thread::Builder::new()
      .name("bridgething-speech".to_owned())
      .spawn(move || run(speaking, rx))
    {
      Ok(_) => {
        let _ = tx.send(command);
        *held = Some(tx);
      }
      Err(error) => {
        *held = None;
        tracing::warn!(%error, "the speech engine could not be started");
        self.speaking.finish(false);
      }
    }
  }
}

impl AudioBackend for SpeechDispatcher {
  fn speak(&self, _id: String, text: String, voice: Option<String>, sink: Arc<SpeakSink>) {
    self.speaking.begin(sink, &text);
    self.send(Command::Speak { text, voice });
  }

  fn cancel(&self, _id: String) {
    self.cancel_all();
  }

  fn cancel_all(&self) {
    self.send(Command::Cancel);
  }

  fn play_earcon(&self, name: String, sink: Arc<EarconSink>) {
    tracing::debug!(%name, "no earcon assets ship with the desktop shell");
    sink.on_finished(false);
  }
}

fn run(speaking: Arc<Utterances>, commands: Receiver<Command>) {
  let Some(mut socket) = connect() else {
    tracing::warn!("speech dispatcher is not listening; nothing on this desktop speaks");
    speaking.finish(false);
    return;
  };

  let (replies, answered) = channel();
  let events = Arc::clone(&speaking);
  let listening = socket.try_clone().and_then(|reading| {
    thread::Builder::new()
      .name("bridgething-speech-events".to_owned())
      .spawn(move || listen(reading, replies, events))
  });
  if let Err(error) = listening {
    tracing::warn!(%error, "the speech socket could not be listened to; nothing on this desktop speaks");
    speaking.finish(false);
    let _ = socket.shutdown(Shutdown::Both);
    return;
  }

  for line in [
    format!("SET self CLIENT_NAME {CLIENT_NAME}"),
    "SET self NOTIFICATION all on".to_owned(),
  ] {
    if !request(&mut socket, &answered, &line) {
      tracing::warn!(%line, "speech dispatcher refused the handshake");
      speaking.finish(false);
      let _ = socket.shutdown(Shutdown::Both);
      return;
    }
  }

  while let Ok(command) = commands.recv() {
    match command {
      Command::Speak { text, voice } => speak(&mut socket, &answered, &speaking, text, voice),
      Command::Cancel => {
        request(&mut socket, &answered, "CANCEL self");
      }
    }
  }

  let _ = socket.write_all(b"QUIT\r\n");
  let _ = socket.shutdown(Shutdown::Both);
}

fn speak(
  socket: &mut UnixStream,
  answered: &Receiver<String>,
  speaking: &Utterances,
  text: String,
  voice: Option<String>,
) {
  if let Some(voice) = voice
    && !request(socket, answered, &format!("SET self SYNTHESIS_VOICE {voice}"))
  {
    request(socket, answered, &format!("SET self LANGUAGE {voice}"));
  }

  if !request(socket, answered, "SPEAK") {
    tracing::warn!("speech dispatcher would not take an utterance");
    speaking.finish(false);
    return;
  }
  if socket
    .write_all(format!("{}\r\n.\r\n", escaped(&text)).as_bytes())
    .and_then(|()| socket.flush())
    .is_err()
    || !accepted(answered)
  {
    tracing::warn!("speech dispatcher dropped an utterance mid-send");
    speaking.finish(false);
  }
}

fn listen(reading: UnixStream, replies: Sender<String>, speaking: Arc<Utterances>) {
  for line in BufReader::new(reading).lines() {
    let Ok(line) = line else { break };
    if line.starts_with(BEGIN) {
      speaking.started();
    } else if line.starts_with(END) {
      speaking.finish(true);
    } else if line.starts_with(CANCELED) {
      speaking.finish(false);
    } else if line.as_bytes().get(3) == Some(&b' ') && replies.send(line).is_err() {
      break;
    }
  }
}

fn request(socket: &mut UnixStream, answered: &Receiver<String>, line: &str) -> bool {
  socket
    .write_all(format!("{line}\r\n").as_bytes())
    .and_then(|()| socket.flush())
    .is_ok()
    && accepted(answered)
}

fn accepted(answered: &Receiver<String>) -> bool {
  answered
    .recv_timeout(REPLY_TIMEOUT)
    .is_ok_and(|reply| reply.starts_with('2'))
}

fn escaped(text: &str) -> String {
  text
    .replace('\r', "")
    .lines()
    .map(|line| {
      if line.starts_with('.') {
        format!(".{line}")
      } else {
        line.to_owned()
      }
    })
    .collect::<Vec<_>>()
    .join("\r\n")
}

fn connect() -> Option<UnixStream> {
  candidates().into_iter().find_map(|path| UnixStream::connect(path).ok())
}

fn candidates() -> Vec<PathBuf> {
  let named = std::env::var_os("SPEECHD_SOCKET").map(PathBuf::from);
  let runtime =
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("speech-dispatcher/speechd.sock"));
  let cached = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/speech-dispatcher/speechd.sock"));
  [named, runtime, cached].into_iter().flatten().collect()
}
