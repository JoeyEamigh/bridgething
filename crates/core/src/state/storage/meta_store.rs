use sea_orm::{DatabaseConnection, EntityTrait, Set};
use uuid::Uuid;

use super::{
  super::{StateResult, webapps::WebappRegistry},
  meta::{Column as MetaColumn, Entity as MetaEntity, KEY_ACTIVE_WEBAPP, KEY_LAUNCHER_WEBAPP, KEY_OVERLAY_WEBAPP},
};

#[derive(Debug, Clone)]
pub struct MetaStore {
  db: DatabaseConnection,
}

#[derive(Debug, Clone, Copy)]
pub struct SlotsReleased {
  pub launcher: bool,
  pub overlay: bool,
}

impl MetaStore {
  pub fn new(db: DatabaseConnection) -> Self {
    Self { db }
  }

  pub async fn active_webapp(&self, webapps: &WebappRegistry) -> StateResult<Option<Uuid>> {
    let stored = self.read_meta(KEY_ACTIVE_WEBAPP).await?;
    let parsed = stored.as_deref().and_then(|s| Uuid::parse_str(s).ok());
    if let Some(id) = parsed
      && webapps.has_app_entry(id).await
    {
      return Ok(Some(id));
    }
    self.launcher_webapp(webapps).await
  }

  pub async fn launcher_slot(&self, webapps: &WebappRegistry) -> StateResult<Option<Uuid>> {
    let Some(id) = self.read_slot(KEY_LAUNCHER_WEBAPP).await? else {
      return Ok(None);
    };
    Ok(webapps.is_launcher(id).await.then_some(id))
  }

  pub async fn overlay_slot(&self, webapps: &WebappRegistry) -> StateResult<Option<Uuid>> {
    let Some(id) = self.read_slot(KEY_OVERLAY_WEBAPP).await? else {
      return Ok(None);
    };
    Ok(webapps.provides_overlay(id).await.then_some(id))
  }

  pub async fn launcher_webapp(&self, webapps: &WebappRegistry) -> StateResult<Option<Uuid>> {
    match self.launcher_slot(webapps).await? {
      Some(id) => Ok(Some(id)),
      None => Ok(webapps.default_id().await),
    }
  }

  pub async fn set_launcher_slot(&self, id: Option<Uuid>) -> StateResult<()> {
    self.write_slot(KEY_LAUNCHER_WEBAPP, id).await
  }

  pub async fn set_overlay_slot(&self, id: Option<Uuid>) -> StateResult<()> {
    self.write_slot(KEY_OVERLAY_WEBAPP, id).await
  }

  pub async fn release_slots_for(&self, id: Uuid) -> StateResult<SlotsReleased> {
    let launcher = self.read_slot(KEY_LAUNCHER_WEBAPP).await? == Some(id);
    if launcher {
      self.write_slot(KEY_LAUNCHER_WEBAPP, None).await?;
    }
    let overlay = self.read_slot(KEY_OVERLAY_WEBAPP).await? == Some(id);
    if overlay {
      self.write_slot(KEY_OVERLAY_WEBAPP, None).await?;
    }
    Ok(SlotsReleased { launcher, overlay })
  }

  async fn read_slot(&self, key: &str) -> StateResult<Option<Uuid>> {
    Ok(
      self
        .read_meta(key)
        .await?
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok()),
    )
  }

  async fn write_slot(&self, key: &str, id: Option<Uuid>) -> StateResult<()> {
    match id {
      Some(id) => self.write_meta(key, &id.simple().to_string()).await,
      None => {
        MetaEntity::delete_by_id(key.to_string()).exec(&self.db).await?;
        Ok(())
      }
    }
  }

  pub async fn set_active_webapp(&self, id: Uuid) -> StateResult<()> {
    self.write_meta(KEY_ACTIVE_WEBAPP, &id.simple().to_string()).await
  }

  pub async fn enforce_active_webapp_exists(&self, webapps: &WebappRegistry) -> StateResult<()> {
    let stored = self.read_meta(KEY_ACTIVE_WEBAPP).await?;
    let parsed = stored.as_deref().and_then(|s| Uuid::parse_str(s).ok());
    if let Some(id) = parsed
      && webapps.has_app_entry(id).await
    {
      return Ok(());
    }
    match self.launcher_webapp(webapps).await? {
      Some(id) => {
        tracing::warn!(
          "persisted active webapp ({:?}) is gone or has no app entry to show; falling back to {}",
          stored,
          id
        );
        self.write_meta(KEY_ACTIVE_WEBAPP, &id.simple().to_string()).await?;
      }
      None => {
        tracing::warn!("no webapps installed and no builtin available; clearing active webapp meta");
        MetaEntity::delete_by_id(KEY_ACTIVE_WEBAPP.to_string())
          .exec(&self.db)
          .await?;
      }
    }
    Ok(())
  }

  async fn read_meta(&self, key: &str) -> StateResult<Option<String>> {
    Ok(
      MetaEntity::find_by_id(key.to_string())
        .one(&self.db)
        .await?
        .map(|m| m.value),
    )
  }

  async fn write_meta(&self, key: &str, value: &str) -> StateResult<()> {
    let model = super::meta::ActiveModel {
      key: Set(key.to_string()),
      value: Set(value.to_string()),
    };
    MetaEntity::insert(model)
      .on_conflict(
        sea_orm::sea_query::OnConflict::column(MetaColumn::Key)
          .update_column(MetaColumn::Value)
          .to_owned(),
      )
      .exec(&self.db)
      .await?;
    Ok(())
  }
}
