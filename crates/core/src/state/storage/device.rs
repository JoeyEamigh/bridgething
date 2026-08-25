use libbridgething::{Device, DeviceType, LinkKind};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "devices")]
pub struct Model {
  #[sea_orm(primary_key, auto_increment = false)]
  pub id: String,
  pub name: String,
  pub device_type: String,
  pub kind: String,
  pub is_default: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl From<&Model> for Device {
  fn from(m: &Model) -> Self {
    Device {
      name: m.name.clone(),
      device_type: parse_device_type(&m.device_type),
      id: m.id.clone(),
      kind: parse_link_kind(&m.kind),
      default: m.is_default,
    }
  }
}

impl Model {
  pub fn from_wire(d: &Device) -> Self {
    Self {
      id: d.id.clone(),
      name: d.name.clone(),
      device_type: device_type_str(&d.device_type).to_string(),
      kind: link_kind_str(d.kind).to_string(),
      is_default: d.default,
    }
  }
}

fn device_type_str(t: &DeviceType) -> &'static str {
  match t {
    DeviceType::Android => "android",
    DeviceType::Ios => "ios",
    DeviceType::Windows => "windows",
    DeviceType::MacOS => "macos",
    DeviceType::Linux => "linux",
    DeviceType::Unknown => "unknown",
  }
}

fn parse_device_type(s: &str) -> DeviceType {
  match s {
    "android" => DeviceType::Android,
    "ios" => DeviceType::Ios,
    "windows" => DeviceType::Windows,
    "macos" => DeviceType::MacOS,
    "linux" => DeviceType::Linux,
    _ => DeviceType::Unknown,
  }
}

pub fn link_kind_str(k: LinkKind) -> &'static str {
  match k {
    LinkKind::Bluetooth => "bluetooth",
    LinkKind::Network => "network",
  }
}

fn parse_link_kind(s: &str) -> LinkKind {
  match s {
    "network" => LinkKind::Network,
    _ => LinkKind::Bluetooth,
  }
}
