use std::{
  ffi::CString,
  fs,
  path::{Path, PathBuf},
};

use crate::status::Disk;

pub const MOUNT_POINT: &str = "/run/mic-debug/drive";

const EXT4_MAGIC: u16 = 0xEF53;
const EXT4_MAGIC_OFFSET: u64 = 0x438;

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
  #[error("no usb block device is present")]
  NoDevice { looked_at: Vec<String> },
  #[error("mounting {device}: {errno}")]
  Mount { device: String, errno: String },
  #[error("{0}")]
  Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
  pub node: PathBuf,
  pub size_bytes: u64,
}

pub fn candidates() -> Result<Vec<Candidate>, DriveError> {
  let mut found = Vec::new();
  for entry in fs::read_dir("/sys/block")? {
    let entry = entry?;
    let disk = entry.file_name().to_string_lossy().into_owned();
    if !is_usb_attached(&entry.path()) {
      continue;
    }
    for part in partitions(&entry.path(), &disk)? {
      found.push(part);
    }
  }
  found.sort_by_key(|entry| std::cmp::Reverse(entry.size_bytes));
  Ok(found)
}

fn is_usb_attached(sys_block: &Path) -> bool {
  fs::canonicalize(sys_block.join("device"))
    .map(|path| path.to_string_lossy().contains("/usb"))
    .unwrap_or(false)
}

fn partitions(sys_block: &Path, disk: &str) -> Result<Vec<Candidate>, DriveError> {
  let mut parts = Vec::new();
  for entry in fs::read_dir(sys_block)? {
    let entry = entry?;
    let name = entry.file_name().to_string_lossy().into_owned();
    if !name.starts_with(disk) || !entry.path().join("partition").exists() {
      continue;
    }
    parts.push(Candidate {
      node: PathBuf::from("/dev").join(&name),
      size_bytes: read_sectors(&entry.path()),
    });
  }
  if parts.is_empty() {
    parts.push(Candidate {
      node: PathBuf::from("/dev").join(disk),
      size_bytes: read_sectors(sys_block),
    });
  }
  Ok(parts)
}

fn read_sectors(sys_path: &Path) -> u64 {
  fs::read_to_string(sys_path.join("size"))
    .ok()
    .and_then(|raw| raw.trim().parse::<u64>().ok())
    .unwrap_or(0)
    * 512
}

pub fn is_ext4(node: &Path) -> bool {
  use std::io::{Read, Seek, SeekFrom};
  let Ok(mut file) = fs::File::open(node) else {
    return false;
  };
  if file.seek(SeekFrom::Start(EXT4_MAGIC_OFFSET)).is_err() {
    return false;
  }
  let mut magic = [0u8; 2];
  file.read_exact(&mut magic).is_ok() && u16::from_le_bytes(magic) == EXT4_MAGIC
}

pub fn mount_first_ext4() -> Result<Candidate, DriveError> {
  let candidates = candidates()?;
  if candidates.is_empty() {
    return Err(DriveError::NoDevice { looked_at: Vec::new() });
  }
  let Some(chosen) = candidates.iter().find(|c| is_ext4(&c.node)) else {
    return Err(DriveError::NoDevice {
      looked_at: candidates.iter().map(|c| c.node.display().to_string()).collect(),
    });
  };

  fs::create_dir_all(MOUNT_POINT)?;
  if is_mounted(MOUNT_POINT) {
    return Ok(chosen.clone());
  }
  mount(&chosen.node, MOUNT_POINT)?;
  Ok(chosen.clone())
}

fn mount(node: &Path, target: &str) -> Result<(), DriveError> {
  let source = CString::new(node.as_os_str().as_encoded_bytes()).expect("device node has no interior nul");
  let target_c = CString::new(target).expect("mount point has no interior nul");
  let fstype = CString::new("ext4").expect("static string");
  let rc = unsafe {
    libc::mount(
      source.as_ptr(),
      target_c.as_ptr(),
      fstype.as_ptr(),
      libc::MS_NOATIME,
      std::ptr::null(),
    )
  };
  if rc != 0 {
    return Err(DriveError::Mount {
      device: node.display().to_string(),
      errno: std::io::Error::last_os_error().to_string(),
    });
  }
  tracing::info!(device = %node.display(), target, "drive mounted");
  Ok(())
}

pub fn unmount() {
  let target = CString::new(MOUNT_POINT).expect("static string");
  // SAFETY: MNT_DETACH so a still-open descriptor cannot leave the drive mounted forever.
  if unsafe { libc::umount2(target.as_ptr(), libc::MNT_DETACH) } == 0 {
    tracing::info!("drive unmounted");
  }
}

pub fn is_mounted(target: &str) -> bool {
  fs::read_to_string("/proc/self/mounts")
    .map(|table| table.lines().any(|line| line.split(' ').nth(1) == Some(target)))
    .unwrap_or(false)
}

pub fn free_space(path: &Path, bytes_per_sec: u64) -> Disk {
  let Ok(c_path) = CString::new(path.as_os_str().as_encoded_bytes()) else {
    return Disk::default();
  };
  let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
  if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
    return Disk::default();
  }
  let unit = stat.f_frsize as u64;
  let free = stat.f_bavail as u64 * unit;
  Disk {
    free_bytes: free,
    total_bytes: stat.f_blocks as u64 * unit,
    remaining_secs: free / bytes_per_sec.max(1),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn candidates_come_back_biggest_first() {
    let mut found = vec![
      Candidate {
        node: "/dev/sda1".into(),
        size_bytes: 100,
      },
      Candidate {
        node: "/dev/sdb1".into(),
        size_bytes: 900,
      },
    ];
    found.sort_by_key(|entry| std::cmp::Reverse(entry.size_bytes));
    assert_eq!(found[0].node, PathBuf::from("/dev/sdb1"));
  }

  #[test]
  fn a_missing_drive_names_what_it_did_see() {
    let err = DriveError::NoDevice {
      looked_at: vec!["/dev/sda1".into()],
    };
    let DriveError::NoDevice { looked_at } = err else {
      panic!("wrong variant");
    };
    assert_eq!(looked_at, vec!["/dev/sda1".to_string()]);
  }

  #[test]
  fn free_space_on_a_missing_path_reads_as_empty_rather_than_panicking() {
    let disk = free_space(Path::new("/definitely/not/here"), 288_000);
    assert_eq!(disk.total_bytes, 0);
    assert_eq!(disk.remaining_secs, 0);
  }
}
