use libbridgething::{BrightnessMode, DimSettings, DisplaySettings};
use sea_orm::{DatabaseConnection, EntityTrait, IntoActiveModel, sea_query::OnConflict};

use super::{
  super::StateResult,
  display_settings::{Column, Entity, Model, ROW_ID},
};

#[derive(Debug, Clone)]
pub struct DisplaySettingsStore {
  db: DatabaseConnection,
}

impl DisplaySettingsStore {
  pub fn new(db: DatabaseConnection) -> Self {
    Self { db }
  }

  pub async fn load(&self) -> StateResult<Option<DisplaySettings>> {
    Ok(
      Entity::find_by_id(ROW_ID)
        .one(&self.db)
        .await?
        .map(|row| DisplaySettings {
          mode: match row.brightness_mode.as_str() {
            "manual" => BrightnessMode::Manual,
            _ => BrightnessMode::Auto,
          },
          level: row.brightness_level,
          dim: DimSettings {
            after_s: row.dim_after_s,
            level: row.dim_level,
          },
        }),
    )
  }

  pub async fn save(&self, prefs: &DisplaySettings) -> StateResult<()> {
    let row = Model {
      id: ROW_ID,
      brightness_mode: match prefs.mode {
        BrightnessMode::Auto => "auto",
        BrightnessMode::Manual => "manual",
      }
      .to_string(),
      brightness_level: prefs.level,
      dim_after_s: prefs.dim.after_s,
      dim_level: prefs.dim.level,
    };
    Entity::insert(row.into_active_model())
      .on_conflict(
        OnConflict::column(Column::Id)
          .update_columns([
            Column::BrightnessMode,
            Column::BrightnessLevel,
            Column::DimAfterS,
            Column::DimLevel,
          ])
          .to_owned(),
      )
      .exec(&self.db)
      .await?;
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn a_fresh_database_has_no_stored_prefs_and_a_save_round_trips() {
    let store = DisplaySettingsStore::new(crate::db::open(None).await.expect("in-memory db"));
    assert_eq!(store.load().await.expect("load"), None);

    let chosen = DisplaySettings {
      mode: BrightnessMode::Manual,
      level: 0.42,
      dim: DimSettings {
        after_s: Some(120),
        level: 0.0,
      },
    };
    store.save(&chosen).await.expect("save");
    assert_eq!(store.load().await.expect("load"), Some(chosen));

    let changed = DisplaySettings {
      dim: DimSettings {
        after_s: None,
        ..chosen.dim
      },
      ..chosen
    };
    store.save(&changed).await.expect("overwrite");
    assert_eq!(store.load().await.expect("load"), Some(changed));
  }
}
