pub fn norm(s: &str) -> String {
  let lowered = s.to_lowercase().replace('&', " and ");
  let tokens = lowered.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty());
  merge_number_words(tokens).join(" ")
}

fn number_word(token: &str) -> Option<u32> {
  let units = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
  ];
  let tens = [
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
  ];
  if let Some(n) = units.iter().position(|u| *u == token) {
    return Some(n as u32);
  }
  tens.iter().position(|t| *t == token).map(|n| (n as u32 + 2) * 10)
}

fn merge_number_words<'a>(tokens: impl Iterator<Item = &'a str>) -> Vec<String> {
  let mut out: Vec<String> = Vec::new();
  let mut pending_tens: Option<u32> = None;
  for token in tokens {
    match (pending_tens.take(), number_word(token)) {
      (Some(tens), Some(unit)) if (1..=9).contains(&unit) => out.push((tens + unit).to_string()),
      (carried, spoken) => {
        if let Some(tens) = carried {
          out.push(tens.to_string());
        }
        match spoken {
          Some(n) if n >= 20 && n % 10 == 0 => pending_tens = Some(n),
          Some(n) => out.push(n.to_string()),
          None => out.push(token.to_string()),
        }
      }
    }
  }
  if let Some(tens) = pending_tens {
    out.push(tens.to_string());
  }
  out
}

pub(crate) fn era_years(era: &str) -> Option<(i32, i32)> {
  let normalized = norm(era);
  let e = normalized.strip_prefix("the ").unwrap_or(&normalized);
  let e = match e {
    "sixties" => "60s",
    "seventies" => "70s",
    "eighties" => "80s",
    "nineties" => "90s",
    other => other,
  };
  if let Ok(year) = e.parse::<i32>() {
    return (1900..=2099).contains(&year).then_some((year, year));
  }
  let decade = e.strip_suffix('s')?.parse::<i32>().ok()?;
  let base = match decade {
    0..=29 => 2000 + decade,
    30..=99 => 1900 + decade,
    1900..=2090 => decade,
    _ => return None,
  };
  (base % 10 == 0).then_some((base, base + 9))
}

pub(crate) fn live_recording(name: &str) -> bool {
  let n = norm(name);
  let padded = format!(" {n} ");
  ["live at", "live in", "live from"]
    .iter()
    .any(|p| padded.contains(&format!(" {p} ")))
    || n.ends_with(" live")
    || padded.contains(" unplugged ")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn normalization_speaks_asr_digits_and_catalog_words_alike() {
    assert_eq!(norm("Twenty One Pilots"), "21 pilots");
    assert_eq!(norm("21 Pilots"), "21 pilots");
    assert_eq!(norm("Blink-182"), "blink 182");
    assert_eq!(norm("Thirty Seconds To Mars"), "30 seconds to mars");
    assert_eq!(norm("Florence & The Machine"), "florence and the machine");
    assert_eq!(norm("Seventy"), "70", "a bare tens word is not swallowed");
    assert_eq!(norm("Twenty Twenty"), "20 20", "tens never merge with tens");
    assert_eq!(norm("One Direction"), "1 direction");
    assert_eq!(norm("  Trench  "), "trench");
  }

  #[test]
  fn eras_parse_as_year_ranges_or_not_at_all() {
    assert_eq!(era_years("2009"), Some((2009, 2009)));
    assert_eq!(era_years("80s"), Some((1980, 1989)));
    assert_eq!(era_years("the 80s"), Some((1980, 1989)));
    assert_eq!(era_years("eighties"), Some((1980, 1989)));
    assert_eq!(era_years("1980s"), Some((1980, 1989)));
    assert_eq!(era_years("20s"), Some((2020, 2029)), "a bare 20s is this century");
    assert_eq!(era_years("2000s"), Some((2000, 2009)));
    assert_eq!(era_years("chill"), None);
    assert_eq!(era_years("85s"), None, "not a decade");
    assert_eq!(era_years("1850"), None, "outside the catalog era");
  }

  #[test]
  fn live_detection_reads_recording_shapes_not_the_word_alone() {
    for name in [
      "More Than We Ever Imagined (Live in Mexico City)",
      "Live at Wembley",
      "MTV Unplugged (Live)",
      "Alchemy Live",
      "Live From the Fillmore",
    ] {
      assert!(live_recording(name), "{name:?} is a live recording");
    }
    for name in ["Live Through This", "Alive", "Living Things", "Breach", "Lively"] {
      assert!(!live_recording(name), "{name:?} is not a live recording");
    }
  }
}
