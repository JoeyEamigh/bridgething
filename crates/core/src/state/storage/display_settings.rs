use sea_orm::entity::prelude::*;

pub const ROW_ID: i32 = 1;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "display_settings")]
pub struct Model {
  #[sea_orm(primary_key, auto_increment = false)]
  pub id: i32,
  pub brightness_mode: String,
  pub brightness_level: f32,
  pub dim_after_s: Option<u32>,
  pub dim_level: f32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
