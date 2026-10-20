use std::panic::AssertUnwindSafe;

use objc2::{exception, rc::autoreleasepool};

const NAMELESS: &str = "an objective-c exception with no object";

pub fn contained<R>(work: impl FnOnce() -> R) -> Result<R, String> {
  exception::catch(AssertUnwindSafe(|| autoreleasepool(|_| work())))
    .map_err(|thrown| thrown.map_or_else(|| NAMELESS.to_owned(), |thrown| thrown.to_string()))
}

#[cfg(test)]
mod tests {
  use objc2_foundation::{NSArray, NSString};

  use super::*;

  #[test]
  fn a_thrown_exception_comes_back_as_its_reason() {
    let empty = NSArray::<NSString>::new();
    let thrown = contained(|| empty.objectAtIndex(3).to_string()).expect_err("an out of range index throws");
    assert!(thrown.contains("index 3 beyond bounds"), "{thrown}");
  }

  #[test]
  fn work_that_does_not_throw_returns_its_value() {
    assert_eq!(
      contained(|| NSString::from_str("held").to_string()),
      Ok("held".to_owned())
    );
  }
}
