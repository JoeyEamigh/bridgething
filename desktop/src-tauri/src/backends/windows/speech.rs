use std::{
  sync::{
    Arc, Mutex,
    mpsc::{Receiver, SendError, Sender, channel},
  },
  thread,
};

use bridgething_companion::backend::{AudioBackend, EarconSink, SpeakSink};
use windows::{
  Foundation::TypedEventHandler,
  Media::{
    Core::MediaSource,
    Playback::{MediaPlayer, MediaPlayerFailedEventArgs},
    SpeechSynthesis::SpeechSynthesizer,
  },
  Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize},
  core::{HSTRING, IInspectable},
};

use crate::backends::utterance::Utterances;

enum Command {
  Speak { text: String, voice: Option<String> },
  Cancel,
}

#[derive(Default)]
pub struct WinRtAudio {
  speaking: Arc<Utterances>,
  engine: Mutex<Option<Sender<Command>>>,
}

impl WinRtAudio {
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

impl AudioBackend for WinRtAudio {
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
  let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  match build(&speaking) {
    Ok((synth, player)) => {
      while let Ok(command) = commands.recv() {
        match command {
          Command::Speak { text, voice } => speak(&synth, &player, &speaking, &text, voice),
          Command::Cancel => {
            let _ = player.Pause();
            speaking.finish(false);
          }
        }
      }
      let _ = player.Pause();
    }
    Err(error) => {
      tracing::warn!(%error, "windows refused a speech synthesizer; nothing on this desktop speaks");
      speaking.finish(false);
    }
  }
  unsafe { CoUninitialize() };
}

fn build(speaking: &Arc<Utterances>) -> windows::core::Result<(SpeechSynthesizer, MediaPlayer)> {
  let synth = SpeechSynthesizer::new()?;
  let player = MediaPlayer::new()?;
  player.CommandManager()?.SetIsEnabled(false)?;

  let held = Arc::clone(speaking);
  player.MediaOpened(&TypedEventHandler::<MediaPlayer, IInspectable>::new(move |_, _| {
    held.started();
    Ok(())
  }))?;
  let held = Arc::clone(speaking);
  player.MediaEnded(&TypedEventHandler::<MediaPlayer, IInspectable>::new(move |_, _| {
    held.finish(true);
    Ok(())
  }))?;
  let held = Arc::clone(speaking);
  player.MediaFailed(&TypedEventHandler::<MediaPlayer, MediaPlayerFailedEventArgs>::new(
    move |_, _| {
      held.finish(false);
      Ok(())
    },
  ))?;

  Ok((synth, player))
}

fn speak(synth: &SpeechSynthesizer, player: &MediaPlayer, speaking: &Utterances, text: &str, voice: Option<String>) {
  if let Some(voice) = voice {
    pick(synth, &voice);
  }
  if let Err(error) = play(synth, player, text) {
    tracing::warn!(%error, "windows dropped an utterance");
    speaking.finish(false);
  }
}

fn play(synth: &SpeechSynthesizer, player: &MediaPlayer, text: &str) -> windows::core::Result<()> {
  let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(text))?.join()?;
  let source = MediaSource::CreateFromStream(&stream, &stream.ContentType()?)?;
  player.SetSource(&source)?;
  player.Play()
}

fn pick(synth: &SpeechSynthesizer, wanted: &str) {
  let Ok(voices) = SpeechSynthesizer::AllVoices() else {
    return;
  };
  let picked = voices
    .into_iter()
    .find(|voice| voice.Id().is_ok_and(|id| id == wanted) || voice.Language().is_ok_and(|language| language == wanted));
  if let Some(voice) = picked
    && let Err(error) = synth.SetVoice(&voice)
  {
    tracing::warn!(%error, %wanted, "the synthesizer kept its own voice");
  }
}
