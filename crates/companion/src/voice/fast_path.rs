use std::sync::LazyLock;

use libbridgething::{NluAmount, NluDirection, NluPlaybackSpeed, NluRepeatMode, NluSlots, NluView};
use regex::Regex;

macro_rules! re {
  ($pattern:literal) => {{
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("fast path pattern compiles"));
    &*PATTERN
  }};
}

macro_rules! res {
  ($($pattern:literal),+ $(,)?) => {{
    static PATTERNS: LazyLock<Vec<Regex>> =
      LazyLock::new(|| vec![$(Regex::new($pattern).expect("fast path pattern compiles")),+]);
    &**PATTERNS
  }};
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
  pub intent: &'static str,
  pub slots: NluSlots,
}

pub fn match_transcript(transcript: &str) -> Option<Hit> {
  let (raw, tokens) = normalize(transcript);
  if tokens.is_empty() {
    return None;
  }
  let core = tokens
    .iter()
    .filter(|token| !GENERIC.contains(&token.as_str()))
    .map(String::as_str)
    .collect::<Vec<&str>>()
    .join(" ");

  RULES.iter().find_map(|rule| rule(&tokens, &raw, &core))
}

const FILLERS: &[&str] = &[
  "uh", "uhh", "uhhh", "uhhhh", "um", "umm", "hmm", "hmmm", "er", "eh", "ah", "oh", "well", "so", "like", "yeah",
  "yep", "yes", "ok", "okay", "hey", "please", "thanks", "thank", "mean", "wait",
];

const GENERIC: &[&str] = &[
  "the",
  "this",
  "that",
  "these",
  "those",
  "a",
  "an",
  "my",
  "our",
  "its",
  "it",
  "current",
  "currently",
  "some",
  "song",
  "songs",
  "track",
  "tracks",
  "music",
  "playback",
  "tune",
  "tunes",
  "playlist",
  "playlists",
  "can",
  "could",
  "would",
  "will",
  "shall",
  "you",
  "your",
  "i",
  "we",
  "us",
  "let",
  "lets",
  "want",
  "wanna",
  "need",
  "gotta",
  "must",
  "should",
  "may",
  "might",
  "now",
  "immediately",
  "for",
  "me",
  "of",
  "already",
  "right",
];

const WORD_TO_NUMBER: &[(&str, i64)] = &[
  ("zero", 0),
  ("one", 1),
  ("two", 2),
  ("three", 3),
  ("four", 4),
  ("five", 5),
  ("six", 6),
  ("seven", 7),
  ("eight", 8),
  ("nine", 9),
  ("ten", 10),
  ("eleven", 11),
  ("twelve", 12),
  ("thirteen", 13),
  ("fourteen", 14),
  ("fifteen", 15),
  ("sixteen", 16),
  ("seventeen", 17),
  ("eighteen", 18),
  ("nineteen", 19),
  ("twenty", 20),
  ("thirty", 30),
  ("forty", 40),
  ("fifty", 50),
  ("sixty", 60),
  ("seventy", 70),
  ("eighty", 80),
  ("ninety", 90),
  ("hundred", 100),
];

static PRESET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"preset\s+(\w+)").expect("preset pattern compiles"));

static COLLECTION_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"\b(?:playlist|album|queue|everything|all)\b").expect("collection pattern compiles"));

static NAMED_COLLECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"\b(?:playlists?|albums?|stations?|podcasts?|artists?)\b").expect("named collection pattern compiles")
});

const PREVIOUS_LEADS: &[&str] = &[
  "play",
  "go",
  "hear",
  "listen",
  "to",
  "back",
  "one",
  "again",
  "more",
  "time",
  "start",
  "from",
  "beginning",
  "over",
];

fn normalize(transcript: &str) -> (String, Vec<String>) {
  let lowered = transcript.to_lowercase();
  let depunctuated = re!(r"[^\w\s']|_").replace_all(&lowered, " ");
  let tokens: Vec<String> = depunctuated
    .split(' ')
    .filter(|token| !token.is_empty() && !FILLERS.contains(token))
    .map(str::to_owned)
    .collect();
  (tokens.join(" "), tokens)
}

fn has(tokens: &[String], word: &str) -> bool {
  tokens.iter().any(|token| token.as_str() == word)
}

fn core_is_only(core: &str, target: &[&str], leads: &[&[&str]]) -> bool {
  let tokens: Vec<&str> = core.split(' ').filter(|token| !token.is_empty()).collect();
  if !tokens.iter().any(|token| target.contains(token)) {
    return false;
  }
  tokens
    .iter()
    .all(|token| target.contains(token) || leads.iter().any(|group| group.contains(token)))
}

fn parse_int(text: &str) -> Option<i64> {
  let cleaned = text.replace('-', " ").replace("percent", "");
  let cleaned = cleaned.trim();
  if cleaned.is_empty() {
    return None;
  }
  if let Ok(direct) = cleaned.parse::<i64>()
    && (0..=100).contains(&direct)
  {
    return Some(direct);
  }

  let mut total: i64 = 0;
  for part in cleaned.split(' ').filter(|part| !part.is_empty()) {
    let value = WORD_TO_NUMBER.iter().find(|(word, _)| *word == part).map(|(_, v)| *v)?;
    total = if value == 100 {
      total.max(1).checked_mul(100)?
    } else {
      total.checked_add(value)?
    };
  }
  (0..=100).contains(&total).then_some(total)
}

fn capture_group<'t>(text: &'t str, pattern: &Regex) -> Option<&'t str> {
  pattern.captures(text)?.get(1).map(|group| group.as_str())
}

fn hit(intent: &'static str, slots: NluSlots) -> Option<Hit> {
  Some(Hit { intent, slots })
}

fn rule_save_to_preset(tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if !has(tokens, "preset") {
    return None;
  }
  if !has(tokens, "save") && !has(tokens, "store") {
    return None;
  }
  let number = parse_int(capture_group(raw, &PRESET_RE)?)?;
  if !(1..=4).contains(&number) {
    return None;
  }
  hit(
    "PRESET_SAVE",
    NluSlots {
      preset: Some(number.to_string()),
      ..Default::default()
    },
  )
}

fn rule_play_preset(tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if !has(tokens, "preset") {
    return None;
  }
  let lead_ok = tokens
    .first()
    .is_some_and(|first| ["play", "load", "switch", "go", "select"].contains(&first.as_str()))
    || (tokens.len() >= 2 && tokens[0] == "go" && tokens[1] == "to");
  if !lead_ok {
    return None;
  }
  if has(tokens, "save") || has(tokens, "store") {
    return None;
  }
  let number = parse_int(capture_group(raw, &PRESET_RE)?)?;
  if !(1..=4).contains(&number) {
    return None;
  }
  if let Some(at) = raw.find("preset") {
    let trailing = raw[at + "preset".len()..].trim();
    if trailing.split(' ').filter(|word| !word.is_empty()).count() > 2 {
      return None;
    }
  }
  hit(
    "PRESET_PLAY",
    NluSlots {
      preset: Some(number.to_string()),
      ..Default::default()
    },
  )
}

fn rule_volume_absolute(tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if !has(tokens, "volume") && !has(tokens, "level") {
    return None;
  }
  let patterns = res![
    r"(?:set|put)\s+(?:the\s+)?volume\s+(?:to|at)\s+([\w\s-]+?)(?:\s+percent|\s*$|\s+please\b)",
    r"\bvolume\s+([\w\s-]+?)\s+percent\b",
    r"\bvolume\s+(?:to|at)\s+([\w\s-]+?)(?:\s*$|\s+percent)",
    r"volume\s+(?:to|at)?\s*(\d+|[a-z]+)\s*(?:percent)?\s*$",
  ];
  for pattern in patterns {
    if let Some(captured) = capture_group(raw, pattern)
      && let Some(level) = parse_int(captured)
      && (1..=100).contains(&level)
    {
      return hit(
        "SET_VOLUME",
        NluSlots {
          level: Some(level as u32),
          ..Default::default()
        },
      );
    }
  }
  None
}

fn rule_playback_speed(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  let anchors = [
    "speed",
    "faster",
    "slower",
    "normal",
    "double",
    "2x",
    "1.5",
    "1.2",
    "two x",
    "two times",
    "one point",
    "half",
  ];
  if !anchors.iter().any(|anchor| raw.contains(anchor)) {
    return None;
  }

  let speed_rules: [(NluPlaybackSpeed, &[Regex]); 4] = [
    (
      NluPlaybackSpeed::OnePointTwo,
      res![
        r"\bone\s+point\s+two(?:\s+(?:speed|x|times))?\b",
        r"\b1\.2\s*x?\b",
        r"\b(?:play\s+it\s+|speed\s+)faster\b",
        r"\bspeed\s+(?:it\s+)?up\b",
        r"\ba\s+little\s+faster\b",
        r"\bfaster\s+a\s+little\b",
      ],
    ),
    (
      NluPlaybackSpeed::OnePointFive,
      res![
        r"\bone\s+(?:and\s+a\s+)?half(?:\s+speed)?\b",
        r"\b1\.5\s*x?\b",
        r"\bone\s+point\s+five\b",
      ],
    ),
    (
      NluPlaybackSpeed::One,
      res![
        r"\bnormal\s+speed\b",
        r"\b(?:back\s+to\s+|reset\s+to\s+)?(?:1\s*x|one\s+x|original\s+speed)\b",
        r"\b(?:play\s+(?:it\s+)?(?:at\s+)?|at\s+)normal(?:\s+speed)?\b",
      ],
    ),
    (
      NluPlaybackSpeed::Two,
      res![
        r"\bdouble\s+speed\b",
        r"\b2\s*x\b",
        r"\btwo\s+x\b",
        r"\btwo\s+times(?:\s+speed)?\b",
      ],
    ),
  ];

  for (speed, patterns) in speed_rules {
    for pattern in patterns {
      if pattern.is_match(raw) {
        return hit(
          "SET_PLAYBACK_SPEED",
          NluSlots {
            speed: Some(speed),
            ..Default::default()
          },
        );
      }
    }
  }
  None
}

fn rule_seek(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  let seek = |seconds: i32| {
    hit(
      "SEEK_RELATIVE",
      NluSlots {
        seconds: Some(seconds),
        ..Default::default()
      },
    )
  };

  let has_fifteen = re!(r"\b(?:15|fifteen)\b").is_match(raw);
  if has_fifteen && re!(r"\b(?:rewind|go\s+back|back|skip\s+back)\b").is_match(raw) {
    return seek(-15);
  }
  if re!(r"^back\s+fifteen\s*$").is_match(raw) {
    return seek(-15);
  }
  if has_fifteen && re!(r"\b(?:fast\s+forward|forward|skip\s+(?:ahead|forward))\b").is_match(raw) {
    return seek(15);
  }
  if re!(r"\bjump\s+ahead\b").is_match(raw)
    && re!(r"\bjump\s+ahead(?:\s+(?:fifteen|15))?\s*(?:seconds?)?\s*$").is_match(raw)
  {
    return seek(15);
  }
  if re!(r"^forward\s+fifteen\s*$").is_match(raw) {
    return seek(15);
  }
  None
}

fn rule_repeat(tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if !has(tokens, "repeat")
    && !has(tokens, "loop")
    && !has(tokens, "looped")
    && !re!(r"\bover\s+and\s+over\b").is_match(raw)
  {
    return None;
  }
  if raw.contains("shuffl") {
    return None;
  }

  let repeat = |mode: NluRepeatMode| {
    hit(
      "SET_REPEAT",
      NluSlots {
        repeat_mode: Some(mode),
        ..Default::default()
      },
    )
  };

  if re!(r"\brepeat\s+off\b").is_match(raw)
    || re!(r"\bstop\s+repeat(?:ing)?\b").is_match(raw)
    || re!(r"\bturn\s+(?:off|of)\s+repeat\b").is_match(raw)
    || re!(r"\bdisable\s+repeat\b").is_match(raw)
    || re!(r"\bstop\s+looping\b").is_match(raw)
  {
    return repeat(NluRepeatMode::Off);
  }

  let whole_collection = COLLECTION_RE.is_match(raw);
  if !whole_collection
    && (re!(r"\b(?:repeat|loop)\s+(?:this(?:\s+(?:song|track|one))?|current(?:\s+(?:song|track))?|it)\b").is_match(raw)
      || re!(r"\bon\s+repeat\b").is_match(raw)
      || re!(r"\b(?:in|on)\s+(?:a\s+)?(?:repeat\s+)?loop\b").is_match(raw)
      || re!(r"\bbe\s+looped\b").is_match(raw)
      || re!(r"\bloop\s+(?:this|that|it)\b").is_match(raw)
      || re!(r"\bover\s+and\s+over\b").is_match(raw))
  {
    return repeat(NluRepeatMode::One);
  }

  if whole_collection && re!(r"\b(?:repeat|loop)\b").is_match(raw) {
    return repeat(NluRepeatMode::All);
  }
  if re!(r"\brepeat(?:\s+on)?\s*$").is_match(raw)
    || matches!(raw, "repeat" | "loop" | "repeat on" | "loop on")
    || re!(r"\bturn\s+on\s+repeat\b").is_match(raw)
    || re!(r"\benable\s+repeat\b").is_match(raw)
  {
    return repeat(NluRepeatMode::All);
  }
  None
}

fn rule_shuffle(tokens: &[String], raw: &str, core: &str) -> Option<Hit> {
  if !has(tokens, "shuffle") && !has(tokens, "shuffling") && !has(tokens, "mix") && !has(tokens, "randomize") {
    return None;
  }
  if (has(tokens, "play") || has(tokens, "start")) && NAMED_COLLECTION_RE.is_match(raw) {
    return None;
  }

  let shuffle = |enabled: bool| {
    hit(
      "SET_SHUFFLE",
      NluSlots {
        enabled: Some(enabled),
        ..Default::default()
      },
    )
  };

  if re!(r"\bshuffle\s+off\b").is_match(raw)
    || re!(r"\bstop\s+shuffling\b").is_match(raw)
    || re!(r"\bturn\s+off\s+shuffle\b").is_match(raw)
    || re!(r"\bdisable\s+shuffle\b").is_match(raw)
  {
    return shuffle(false);
  }
  if core_is_only(
    core,
    &["shuffle", "shuffling", "mix"],
    &[&[
      "on",
      "up",
      "turn",
      "enable",
      "start",
      "play",
      "and",
      "repeat",
      "put",
      "mode",
      "needs",
      "randomize",
    ]],
  ) {
    return shuffle(true);
  }
  None
}

fn rule_whats_playing(tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if tokens.len() > 8 {
    return None;
  }
  let patterns = res![
    r"^what'?s\s+playing\s*$",
    r"^what'?s\s+this(?:\s+song)?\s*$",
    r"^what\s+is\s+this(?:\s+song)?\s*$",
    r"^who'?s\s+(?:this|playing)\s*$",
    r"^who\s+is\s+(?:this|playing)\s*$",
    r"\bname\s+of\s+(?:the\s+|this\s+)?(?:song|track|artist)\b",
    r"^what\s+song\s+is\s+this\s*$",
  ];
  for pattern in patterns {
    if pattern.is_match(raw) {
      return hit(
        "SHOW_VIEW",
        NluSlots {
          view: Some(NluView::NowPlaying),
          ..Default::default()
        },
      );
    }
  }
  None
}

fn mute(muted: bool) -> Option<Hit> {
  hit(
    "SET_VOLUME",
    NluSlots {
      mute: Some(muted),
      ..Default::default()
    },
  )
}

fn rule_unmute(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if re!(r"\bunmute\b").is_match(raw) {
    return mute(false);
  }
  if re!(r"\bturn\s+(?:the\s+)?(?:sound|audio|volume)\s+back\s+on\b").is_match(raw) {
    return mute(false);
  }
  None
}

fn rule_mute(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if re!(r"^mute(?:\s+(?:it|the\s+(?:audio|sound|volume|music)))?\s*$").is_match(raw) {
    return mute(true);
  }
  if re!(r"\bturn\s+(?:off|down\s+to\s+zero)\s+(?:the\s+)?(?:sound|audio|volume)\b").is_match(raw) {
    return mute(true);
  }
  None
}

fn amount_modifier(raw: &str) -> NluAmount {
  if re!(r"\ba\s+(?:little|bit|tiny\s+bit|touch)\b").is_match(raw) {
    return NluAmount::Small;
  }
  if re!(r"\ba\s+lot\b|\bway\b|\bmuch\s+(?:louder|higher|quieter|lower)\b").is_match(raw) {
    return NluAmount::Large;
  }
  NluAmount::Medium
}

fn step_volume(raw: &str, direction: NluDirection) -> Option<Hit> {
  hit(
    "SET_VOLUME",
    NluSlots {
      direction: Some(direction),
      amount: Some(amount_modifier(raw)),
      ..Default::default()
    },
  )
}

fn rule_volume_up(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  let patterns = res![
    r"\bvolume\s+up\b",
    r"^louder\s*$",
    r"\bturn\s+(?:it|the\s+(?:volume|music))?\s*up\b",
    r"\bturn\s+up\s+(?:the\s+)?volume\b",
    r"\bcrank\s+(?:it\s+)?up\b",
    r"^make\s+(?:it\s+)?louder\s*$",
  ];
  for pattern in patterns {
    if pattern.is_match(raw) {
      return step_volume(raw, NluDirection::Up);
    }
  }
  None
}

fn rule_volume_down(_tokens: &[String], raw: &str, _core: &str) -> Option<Hit> {
  if re!(r"\bvolume\s+down\b").is_match(raw)
    || matches!(raw, "quieter" | "softer")
    || re!(r"\bturn\s+(?:it|the\s+(?:volume|music))?\s*down\b").is_match(raw)
    || re!(r"\bturn\s+down\s+(?:the\s+)?volume\b").is_match(raw)
    || re!(r"^make\s+(?:it\s+)?(?:quieter|softer)\s*$").is_match(raw)
  {
    return step_volume(raw, NluDirection::Down);
  }
  None
}

fn rule_play_resume(_tokens: &[String], _raw: &str, core: &str) -> Option<Hit> {
  if core_is_only(core, &["resume"], &[&["playing", "play"]]) {
    return hit("PLAY", NluSlots::default());
  }
  if matches!(core, "keep playing" | "keep going") {
    return hit("PLAY", NluSlots::default());
  }
  None
}

fn rule_pause(_tokens: &[String], _raw: &str, core: &str) -> Option<Hit> {
  if core_is_only(core, &["pause"], &[&["playing"]]) {
    return hit("PAUSE", NluSlots::default());
  }
  None
}

fn rule_pause_stop(tokens: &[String], raw: &str, core: &str) -> Option<Hit> {
  if has(tokens, "repeat") || raw.contains("shuffl") {
    return None;
  }
  if core_is_only(core, &["stop", "end"], &[&["playing", "play", "from"]]) {
    return hit("PAUSE", NluSlots::default());
  }
  if core_is_only(core, &["off"], &[&["turn", "playing", "play"]]) {
    return hit("PAUSE", NluSlots::default());
  }
  None
}

fn rule_next(_tokens: &[String], raw: &str, core: &str) -> Option<Hit> {
  if core.contains("back") || NAMED_COLLECTION_RE.is_match(raw) {
    return None;
  }
  if core_is_only(
    core,
    &["next", "skip"],
    &[&["play", "go", "hear", "listen", "to", "one", "ahead", "forward"]],
  ) {
    return hit("NEXT", NluSlots::default());
  }
  None
}

fn rule_previous(_tokens: &[String], raw: &str, core: &str) -> Option<Hit> {
  if NAMED_COLLECTION_RE.is_match(raw) {
    return None;
  }
  if core_is_only(core, &["previous", "replay"], &[PREVIOUS_LEADS, &["last"]]) {
    return hit("PREVIOUS", NluSlots::default());
  }
  if core_is_only(core, &["last"], &[PREVIOUS_LEADS, &["repeat", "replay"]])
    && re!(r"\b(?:play|go|hear|listen|back|repeat|replay|start)\b").is_match(core)
  {
    return hit("PREVIOUS", NluSlots::default());
  }
  if core_is_only(core, &["back"], &[&["go", "one", "to"]]) {
    return hit("PREVIOUS", NluSlots::default());
  }
  None
}

fn rule_play_bare(_tokens: &[String], _raw: &str, core: &str) -> Option<Hit> {
  if core_is_only(core, &["play", "start"], &[&["playing", "something", "go", "on"]]) {
    return hit("PLAY", NluSlots::default());
  }
  if core == "go" {
    return hit("PLAY", NluSlots::default());
  }
  None
}

type Rule = fn(&[String], &str, &str) -> Option<Hit>;

const RULES: &[Rule] = &[
  rule_save_to_preset,
  rule_play_preset,
  rule_volume_absolute,
  rule_playback_speed,
  rule_seek,
  rule_repeat,
  rule_shuffle,
  rule_whats_playing,
  rule_unmute,
  rule_mute,
  rule_volume_up,
  rule_volume_down,
  rule_play_resume,
  rule_pause,
  rule_pause_stop,
  rule_next,
  rule_previous,
  rule_play_bare,
];
