use std::{
  fs::File,
  io::{self, BufReader, Read, Seek, SeekFrom},
  path::{Path, PathBuf},
};

use crate::ota::slots::Slot;

const SECTOR_BYTES: u64 = 512;
const WRITE_SECTORS_FIELD: usize = 6;
const CPIO_HEADER_LEN: usize = 110;
const CPIO_TRAILER: &str = "TRAILER!!!";
const SW_DESCRIPTION: &str = "sw-description";
const ZCK_MAGIC: &[u8; 5] = b"\0ZCK1";
const ZCK_HEADER_ONLY_MAGIC: &[u8; 5] = b"\0ZHR1";
const ZCK_FLAG_OPTIONAL_ELEMENTS: u64 = 2;
const ZCK_FLAG_UNCOMPRESSED_DIGESTS: u64 = 4;
const ZCK_MAX_COMPINT_BYTES: usize = 10;

pub struct WriteMeter {
  total: u64,
  partitions: Vec<Partition>,
}

impl WriteMeter {
  #[cfg(feature = "swupdate")]
  pub fn open(swu: &Path, target: Slot) -> io::Result<Self> {
    Self::open_under(swu, target, Path::new("/"))
  }

  fn open_under(swu: &Path, target: Slot, root: &Path) -> io::Result<Self> {
    let total = payload_bytes(&mut BufReader::new(File::open(swu)?))?;
    let partitions = partlabels(target)
      .iter()
      .map(|label| Partition::open(root, label))
      .collect::<io::Result<Vec<_>>>()?;
    Ok(Self { total, partitions })
  }

  pub fn percent(&self) -> u8 {
    let written: u64 = self.partitions.iter().map(Partition::written).sum();
    fraction_percent(written, self.total)
  }
}

fn fraction_percent(done: u64, total: u64) -> u8 {
  if total == 0 {
    return 0;
  }
  (u128::from(done) * 100 / u128::from(total)).min(100) as u8
}

fn partlabels(slot: Slot) -> [&'static str; 2] {
  match slot {
    Slot::A => ["boot_a", "root_a"],
    Slot::B => ["boot_b", "root_b"],
  }
}

struct Partition {
  stat: PathBuf,
  baseline: u64,
}

impl Partition {
  fn open(root: &Path, label: &str) -> io::Result<Self> {
    let device = std::fs::canonicalize(root.join("dev/disk/by-partlabel").join(label))?;
    let name = device
      .file_name()
      .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{label} resolves to no block device")))?;
    let stat = root.join("sys/class/block").join(name).join("stat");
    let baseline = written_sectors(&std::fs::read_to_string(&stat)?)?;
    Ok(Self { stat, baseline })
  }

  fn written(&self) -> u64 {
    std::fs::read_to_string(&self.stat)
      .ok()
      .and_then(|stat| written_sectors(&stat).ok())
      .map_or(0, |sectors| sectors.saturating_sub(self.baseline) * SECTOR_BYTES)
  }
}

fn written_sectors(stat: &str) -> io::Result<u64> {
  stat
    .split_whitespace()
    .nth(WRITE_SECTORS_FIELD)
    .and_then(|field| field.parse().ok())
    .ok_or_else(|| invalid("block stat has no write sector count"))
}

fn payload_bytes<R: Read + Seek>(swu: &mut R) -> io::Result<u64> {
  let mut total = 0u64;
  loop {
    let mut header = [0u8; CPIO_HEADER_LEN];
    swu.read_exact(&mut header)?;
    if &header[..5] != b"07070" || !matches!(header[5], b'1' | b'2') {
      return Err(invalid("not a newc cpio archive"));
    }
    let size = cpio_field(&header, 6)?;
    let name_len = cpio_field(&header, 11)?;
    let mut name = vec![0u8; name_len as usize];
    swu.read_exact(&mut name)?;
    swu.seek(SeekFrom::Current(pad4(CPIO_HEADER_LEN as u64 + name_len) as i64))?;
    let name = String::from_utf8_lossy(name.strip_suffix(&[0]).unwrap_or(&name)).into_owned();
    if name == CPIO_TRAILER {
      return Ok(total);
    }

    let start = swu.stream_position()?;
    if !name.starts_with(SW_DESCRIPTION) {
      total += entry_bytes(&mut (&mut *swu).take(size), size)?;
    }
    swu.seek(SeekFrom::Start(start + size + pad4(size)))?;
  }
}

fn entry_bytes(entry: &mut impl Read, size: u64) -> io::Result<u64> {
  let mut magic = [0u8; 5];
  if size < magic.len() as u64 {
    return Ok(size);
  }
  entry.read_exact(&mut magic)?;
  if &magic == ZCK_MAGIC || &magic == ZCK_HEADER_ONLY_MAGIC {
    zck_uncompressed_bytes(entry)
  } else {
    Ok(size)
  }
}

fn cpio_field(header: &[u8; CPIO_HEADER_LEN], index: usize) -> io::Result<u64> {
  let at = 6 + index * 8;
  std::str::from_utf8(&header[at..at + 8])
    .ok()
    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
    .ok_or_else(|| invalid("cpio header field is not hex"))
}

fn pad4(n: u64) -> u64 {
  (4 - n % 4) % 4
}

fn zck_uncompressed_bytes(zck: &mut impl Read) -> io::Result<u64> {
  let digest = zck_digest_len(compint(zck)?)?;
  compint(zck)?;
  skip(zck, digest)?;

  skip(zck, digest)?;
  let flags = compint(zck)?;
  compint(zck)?;
  if flags & ZCK_FLAG_OPTIONAL_ELEMENTS != 0 {
    for _ in 0..compint(zck)? {
      compint(zck)?;
      let len = compint(zck)?;
      skip(zck, len)?;
    }
  }
  compint(zck)?;

  let chunk_digest = zck_digest_len(compint(zck)?)?;
  let entry_digests = if flags & ZCK_FLAG_UNCOMPRESSED_DIGESTS != 0 {
    chunk_digest * 2
  } else {
    chunk_digest
  };
  let mut total = 0u64;
  for _ in 0..compint(zck)? {
    skip(zck, entry_digests)?;
    compint(zck)?;
    total += compint(zck)?;
  }
  Ok(total)
}

fn zck_digest_len(hash_type: u64) -> io::Result<u64> {
  match hash_type {
    0 => Ok(20),
    1 => Ok(32),
    2 => Ok(64),
    3 => Ok(16),
    other => Err(invalid(&format!("unknown zchunk hash type {other}"))),
  }
}

fn compint(r: &mut impl Read) -> io::Result<u64> {
  let mut value = 0u64;
  for at in 0..ZCK_MAX_COMPINT_BYTES {
    let mut byte = [0u8; 1];
    r.read_exact(&mut byte)?;
    value |= u64::from(byte[0] & 0x7f) << (7 * at);
    if byte[0] & 0x80 != 0 {
      return Ok(value);
    }
  }
  Err(invalid("zchunk integer is too long"))
}

fn skip(r: &mut impl Read, n: u64) -> io::Result<()> {
  let skipped = io::copy(&mut r.take(n), &mut io::sink())?;
  if skipped != n {
    return Err(io::ErrorKind::UnexpectedEof.into());
  }
  Ok(())
}

fn invalid(msg: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

#[cfg(test)]
mod tests {
  use std::io::Cursor;

  use super::*;

  fn put_compint(out: &mut Vec<u8>, mut value: u64) {
    while value > 127 {
      out.push((value % 128) as u8);
      value /= 128;
    }
    out.push(value as u8 | 0x80);
  }

  fn zck_header(flags: u64, chunks: &[(u64, u64)]) -> Vec<u8> {
    let mut index = Vec::new();
    put_compint(&mut index, 1);
    put_compint(&mut index, chunks.len() as u64);
    for &(comp, len) in chunks {
      let digests = if flags & ZCK_FLAG_UNCOMPRESSED_DIGESTS != 0 {
        64
      } else {
        32
      };
      index.extend(std::iter::repeat_n(0xab, digests));
      put_compint(&mut index, comp);
      put_compint(&mut index, len);
    }

    let mut preface = vec![0xcd; 32];
    put_compint(&mut preface, flags);
    put_compint(&mut preface, 2);
    if flags & ZCK_FLAG_OPTIONAL_ELEMENTS != 0 {
      put_compint(&mut preface, 1);
      put_compint(&mut preface, 7);
      put_compint(&mut preface, 3);
      preface.extend([1, 2, 3]);
    }
    put_compint(&mut preface, index.len() as u64);

    let mut out = ZCK_MAGIC.to_vec();
    put_compint(&mut out, 1);
    put_compint(&mut out, (preface.len() + index.len()) as u64);
    out.extend([0xef; 32]);
    out.extend(preface);
    out.extend(index);
    out
  }

  fn cpio(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut push = |name: &str, data: &[u8]| {
      let name_len = name.len() + 1;
      out.extend(b"070702");
      for field in [0usize, 0o100644, 0, 0, 1, 0, data.len(), 0, 0, 0, 0, name_len, 0] {
        out.extend(format!("{field:08X}").as_bytes());
      }
      out.extend(name.as_bytes());
      out.push(0);
      out.resize(out.len() + pad4((CPIO_HEADER_LEN + name_len) as u64) as usize, 0);
      out.extend(data);
      out.resize(out.len() + pad4(data.len() as u64) as usize, 0);
    };
    for (name, data) in entries {
      push(name, data);
    }
    push(CPIO_TRAILER, &[]);
    out
  }

  #[test]
  fn a_delta_swu_counts_the_uncompressed_size_of_every_zchunk_image() {
    let swu = cpio(&[
      ("sw-description", b"software = {};".to_vec()),
      (
        "boot.vfat.zck.zckheader",
        zck_header(0, &[(0, 0), (10, 4096), (20, 300_000)]),
      ),
      (
        "system.img.zck.zckheader",
        zck_header(ZCK_FLAG_UNCOMPRESSED_DIGESTS, &[(0, 0), (5, 1 << 30)]),
      ),
    ]);
    assert_eq!(
      payload_bytes(&mut Cursor::new(swu)).unwrap(),
      4096 + 300_000 + (1 << 30)
    );
  }

  #[test]
  fn a_full_swu_counts_each_raw_image_as_shipped() {
    let swu = cpio(&[
      ("sw-description", b"software = {};".to_vec()),
      ("sw-description.sig", vec![9; 256]),
      ("boot.vfat", vec![0; 4099]),
      ("system.img", vec![1; 70_001]),
    ]);
    assert_eq!(payload_bytes(&mut Cursor::new(swu)).unwrap(), 4099 + 70_001);
  }

  #[test]
  fn optional_preface_elements_are_skipped() {
    let header = zck_header(ZCK_FLAG_OPTIONAL_ELEMENTS, &[(0, 0), (1, 777), (1, 223)]);
    assert_eq!(zck_uncompressed_bytes(&mut Cursor::new(&header[5..])).unwrap(), 1000);
  }

  #[test]
  fn a_truncated_zchunk_header_is_an_error_not_a_short_total() {
    let header = zck_header(0, &[(0, 0), (1, 777), (1, 223)]);
    let cut = &header[5..header.len() - 3];
    assert!(zck_uncompressed_bytes(&mut Cursor::new(cut)).is_err());
  }

  #[test]
  fn compints_round_trip_across_byte_boundaries() {
    for value in [0, 1, 127, 128, 16_383, 16_384, 518_500_352, u64::from(u32::MAX) * 3] {
      let mut encoded = Vec::new();
      put_compint(&mut encoded, value);
      assert_eq!(compint(&mut Cursor::new(encoded)).unwrap(), value);
    }
  }

  #[test]
  fn the_write_sector_count_is_the_seventh_stat_field() {
    let stat = "    4321     12   99999   1200    8765     43  1048576   5600    0   7000   6800";
    assert_eq!(written_sectors(stat).unwrap(), 1_048_576);
  }

  fn stat_line(written: u64) -> String {
    format!("    4321     12   99999   1200    8765     43 {written:>8}   5600    0   7000   6800\n")
  }

  #[test]
  fn the_meter_counts_only_what_lands_on_the_target_slot_after_it_opens() {
    let root = std::env::temp_dir().join(format!("bridgething-meter-{}", uuid::Uuid::now_v7()));
    let labels = root.join("dev/disk/by-partlabel");
    std::fs::create_dir_all(&labels).unwrap();
    let stat = |name: &str| root.join("sys/class/block").join(name).join("stat");
    for (label, name, written) in [
      ("boot_a", "mmcblk0p2", 900),
      ("root_a", "mmcblk0p3", 5_000),
      ("boot_b", "mmcblk0p4", 0),
      ("root_b", "mmcblk0p5", 7_000),
    ] {
      std::fs::write(root.join("dev").join(name), b"").unwrap();
      std::os::unix::fs::symlink(format!("../../{name}"), labels.join(label)).unwrap();
      std::fs::create_dir_all(stat(name).parent().unwrap()).unwrap();
      std::fs::write(stat(name), stat_line(written)).unwrap();
    }
    let swu = root.join("update.swu");
    std::fs::write(
      &swu,
      cpio(&[("boot.vfat", vec![0; 512 * 100]), ("system.img", vec![0; 512 * 300])]),
    )
    .unwrap();

    let meter = WriteMeter::open_under(&swu, Slot::B, &root).unwrap();
    assert_eq!(
      meter.percent(),
      0,
      "sectors written before the install are not progress"
    );

    std::fs::write(stat("mmcblk0p4"), stat_line(100)).unwrap();
    std::fs::write(stat("mmcblk0p3"), stat_line(9_999)).unwrap();
    assert_eq!(
      meter.percent(),
      25,
      "the whole boot image is a quarter, and the running slot does not count"
    );

    std::fs::write(stat("mmcblk0p5"), stat_line(7_150)).unwrap();
    assert_eq!(meter.percent(), 62);

    std::fs::write(stat("mmcblk0p5"), stat_line(7_300)).unwrap();
    assert_eq!(meter.percent(), 100);

    let _ = std::fs::remove_dir_all(&root);
  }

  #[test]
  fn percent_never_runs_past_the_whole_install() {
    assert_eq!(fraction_percent(0, 1000), 0);
    assert_eq!(fraction_percent(499, 1000), 49);
    assert_eq!(fraction_percent(1000, 1000), 100);
    assert_eq!(fraction_percent(1100, 1000), 100);
    assert_eq!(fraction_percent(10, 0), 0);
  }
}
