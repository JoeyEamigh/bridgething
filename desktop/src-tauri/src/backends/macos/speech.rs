use std::sync::{Arc, Mutex};

use bridgething_companion::backend::{AudioBackend, EarconSink, SpeakSink};
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, rc::Retained, runtime::ProtocolObject};
use objc2_avf_audio::{
  AVSpeechBoundary, AVSpeechSynthesisVoice, AVSpeechSynthesizer, AVSpeechSynthesizerDelegate, AVSpeechUtterance,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use crate::backends::utterance::Utterances;

struct Engine {
  synth: Retained<AVSpeechSynthesizer>,
  _delegate: Retained<SpeechDelegate>,
}

pub struct AvAudio {
  speaking: Arc<Utterances>,
  engine: Arc<Mutex<Option<MainThreadBound<Engine>>>>,
}

impl AvAudio {
  pub fn new() -> Self {
    Self {
      speaking: Arc::new(Utterances::default()),
      engine: Arc::new(Mutex::new(None)),
    }
  }

  fn on_main(&self, work: impl FnOnce(&AVSpeechSynthesizer) + Send + 'static) {
    let engine = Arc::clone(&self.engine);
    let speaking = Arc::clone(&self.speaking);
    DispatchQueue::main().exec_async(move || {
      let Some(mtm) = MainThreadMarker::new() else { return };
      let mut held = engine.lock().unwrap();
      let bound = held.get_or_insert_with(|| MainThreadBound::new(build(speaking, mtm), mtm));
      work(&bound.get(mtm).synth);
    });
  }
}

impl AudioBackend for AvAudio {
  fn speak(&self, _id: String, text: String, voice: Option<String>, sink: Arc<SpeakSink>) {
    self.speaking.begin(sink, &text);
    self.on_main(move |synth| unsafe {
      let utterance = AVSpeechUtterance::initWithString(AVSpeechUtterance::alloc(), &NSString::from_str(&text));
      if let Some(voice) = voice {
        let wanted = NSString::from_str(&voice);
        let picked = AVSpeechSynthesisVoice::voiceWithIdentifier(&wanted)
          .or_else(|| AVSpeechSynthesisVoice::voiceWithLanguage(Some(&wanted)));
        utterance.setVoice(picked.as_deref());
      }
      synth.speakUtterance(&utterance);
    });
  }

  fn cancel(&self, _id: String) {
    self.cancel_all();
  }

  fn cancel_all(&self) {
    self.on_main(|synth| unsafe {
      synth.stopSpeakingAtBoundary(AVSpeechBoundary::Immediate);
    });
  }

  fn play_earcon(&self, name: String, sink: Arc<EarconSink>) {
    tracing::debug!(%name, "no earcon assets ship with the desktop shell");
    sink.on_finished(false);
  }
}

fn build(speaking: Arc<Utterances>, _mtm: MainThreadMarker) -> Engine {
  let delegate = SpeechDelegate::new(speaking);
  let synth = unsafe { AVSpeechSynthesizer::new() };
  unsafe { synth.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
  Engine {
    synth,
    _delegate: delegate,
  }
}

struct SpeechState {
  speaking: Arc<Utterances>,
}

define_class!(
  // SAFETY: NSObject has no subclassing requirements and this class has no Drop.
  #[unsafe(super(NSObject))]
  #[ivars = SpeechState]
  struct SpeechDelegate;

  unsafe impl NSObjectProtocol for SpeechDelegate {}

  unsafe impl AVSpeechSynthesizerDelegate for SpeechDelegate {
    #[unsafe(method(speechSynthesizer:didStartSpeechUtterance:))]
    fn did_start(&self, _synth: &AVSpeechSynthesizer, _utterance: &AVSpeechUtterance) {
      self.ivars().speaking.started();
    }

    #[unsafe(method(speechSynthesizer:didFinishSpeechUtterance:))]
    fn did_finish(&self, _synth: &AVSpeechSynthesizer, _utterance: &AVSpeechUtterance) {
      self.ivars().speaking.finish(true);
    }

    #[unsafe(method(speechSynthesizer:didCancelSpeechUtterance:))]
    fn did_cancel(&self, _synth: &AVSpeechSynthesizer, _utterance: &AVSpeechUtterance) {
      self.ivars().speaking.finish(false);
    }
  }
);

impl SpeechDelegate {
  fn new(speaking: Arc<Utterances>) -> Retained<Self> {
    let this = Self::alloc().set_ivars(SpeechState { speaking });
    unsafe { msg_send![super(this), init] }
  }
}
