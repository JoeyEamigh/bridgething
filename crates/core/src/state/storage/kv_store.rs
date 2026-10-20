use libbridgething::WebappManifest;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TransactionTrait};
use uuid::Uuid;

use super::{
  super::{StateError, StateResult},
  kv_storage::{Column as KvColumn, Entity as KvEntity},
};

#[derive(Debug, Clone)]
pub struct KvStore {
  db: DatabaseConnection,
}

impl KvStore {
  pub fn new(db: DatabaseConnection) -> Self {
    Self { db }
  }

  pub async fn data_get(&self, app_id: Uuid, key: &str) -> StateResult<Option<String>> {
    self.read_raw(&data_namespace_key(app_id, key)).await
  }

  pub async fn data_set(&self, app_id: Uuid, key: &str, value: String) -> StateResult<()> {
    self.write_raw(data_namespace_key(app_id, key), value).await
  }

  pub async fn data_delete(&self, app_id: Uuid, key: &str) -> StateResult<()> {
    self.delete_raw(&data_namespace_key(app_id, key)).await
  }

  pub async fn data_list_prefix(&self, app_id: Uuid, key_prefix: &str) -> StateResult<Vec<(String, String)>> {
    let full_prefix = format!("{}:data:{key_prefix}", app_id.simple());
    let pattern = format!("{full_prefix}%");
    let rows = KvEntity::find()
      .filter(KvColumn::Key.like(&pattern))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(
      rows
        .into_iter()
        .filter_map(|m| m.key.strip_prefix(&full_prefix).map(|k| (k.to_string(), m.value)))
        .collect(),
    )
  }

  pub async fn data_set_many(
    &self,
    app_id: Uuid,
    items: impl IntoIterator<Item = (String, String)>,
  ) -> StateResult<()> {
    let tx = self.db.begin().await?;
    for (key, value) in items {
      let model = super::kv_storage::ActiveModel {
        key: Set(data_namespace_key(app_id, &key)),
        value: Set(value),
      };
      KvEntity::insert(model)
        .on_conflict(
          sea_orm::sea_query::OnConflict::column(KvColumn::Key)
            .update_column(KvColumn::Value)
            .to_owned(),
        )
        .exec(&tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
  }

  pub async fn doc_get(&self, app_id: Uuid, key: &str) -> StateResult<Option<String>> {
    self.read_raw(&doc_namespace_key(app_id, key)).await
  }

  pub async fn doc_set(&self, app_id: Uuid, key: &str, value: String) -> StateResult<()> {
    self.write_raw(doc_namespace_key(app_id, key), value).await
  }

  pub async fn doc_delete(&self, app_id: Uuid, key: &str) -> StateResult<()> {
    self.delete_raw(&doc_namespace_key(app_id, key)).await
  }

  pub async fn doc_list(&self, app_id: Uuid) -> StateResult<Vec<(String, String)>> {
    let prefix = doc_namespace_prefix(app_id);
    let pattern = format!("{prefix}%");
    let rows = KvEntity::find()
      .filter(KvColumn::Key.like(&pattern))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(
      rows
        .into_iter()
        .filter_map(|m| m.key.strip_prefix(&prefix).map(|k| (k.to_string(), m.value)))
        .collect(),
    )
  }

  pub async fn config_get(&self, app_id: Uuid, key: &str) -> StateResult<Option<String>> {
    self.read_raw(&config_namespace_key(app_id, key)).await
  }

  pub async fn config_set(&self, app_id: Uuid, key: &str, value: String) -> StateResult<()> {
    self.write_raw(config_namespace_key(app_id, key), value).await
  }

  pub async fn config_delete(&self, app_id: Uuid, key: &str) -> StateResult<()> {
    self.delete_raw(&config_namespace_key(app_id, key)).await
  }

  pub async fn blob_get(&self, app_id: Uuid, key: &str) -> StateResult<Option<String>> {
    self.read_raw(&blob_namespace_key(app_id, key)).await
  }

  pub async fn blob_set(&self, app_id: Uuid, key: &str, value: String) -> StateResult<()> {
    self.write_raw(blob_namespace_key(app_id, key), value).await
  }

  pub async fn blob_delete(&self, app_id: Uuid, key: &str) -> StateResult<()> {
    self.delete_raw(&blob_namespace_key(app_id, key)).await
  }

  pub async fn blob_list(&self, app_id: Uuid) -> StateResult<Vec<(String, String)>> {
    let prefix = blob_namespace_prefix(app_id);
    let pattern = format!("{prefix}%");
    let rows = KvEntity::find()
      .filter(KvColumn::Key.like(&pattern))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(
      rows
        .into_iter()
        .filter_map(|m| m.key.strip_prefix(&prefix).map(|k| (k.to_string(), m.value)))
        .collect(),
    )
  }

  pub async fn blob_rows(&self) -> StateResult<Vec<(Uuid, String, String)>> {
    let pattern = format!("{}:blob:%", "_".repeat(uuid::fmt::Simple::LENGTH));
    let rows = KvEntity::find()
      .filter(KvColumn::Key.like(&pattern))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(
      rows
        .into_iter()
        .filter_map(|m| {
          let (app_id, key) = m.key.split_once(":blob:")?;
          Some((Uuid::try_parse(app_id).ok()?, key.to_string(), m.value))
        })
        .collect(),
    )
  }

  pub async fn blob_purge(&self, app_id: Uuid) -> StateResult<()> {
    let pattern = format!("{}%", blob_namespace_prefix(app_id));
    KvEntity::delete_many()
      .filter(KvColumn::Key.like(&pattern))
      .exec(&self.db)
      .await?;
    Ok(())
  }

  async fn read_raw(&self, key: &str) -> StateResult<Option<String>> {
    Ok(
      KvEntity::find_by_id(key.to_string())
        .one(&self.db)
        .await?
        .map(|m| m.value),
    )
  }

  async fn write_raw(&self, key: String, value: String) -> StateResult<()> {
    let model = super::kv_storage::ActiveModel {
      key: Set(key),
      value: Set(value),
    };
    KvEntity::insert(model)
      .on_conflict(
        sea_orm::sea_query::OnConflict::column(KvColumn::Key)
          .update_column(KvColumn::Value)
          .to_owned(),
      )
      .exec(&self.db)
      .await?;
    Ok(())
  }

  async fn delete_raw(&self, key: &str) -> StateResult<()> {
    KvEntity::delete_by_id(key.to_string()).exec(&self.db).await?;
    Ok(())
  }

  pub async fn config_list(&self, app_id: Uuid) -> StateResult<Vec<(String, String)>> {
    let prefix = config_namespace_prefix(app_id);
    let pattern = format!("{prefix}%");
    let rows = KvEntity::find()
      .filter(KvColumn::Key.like(&pattern))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(
      rows
        .into_iter()
        .filter_map(|m| m.key.strip_prefix(&prefix).map(|k| (k.to_string(), m.value)))
        .collect(),
    )
  }

  pub async fn seed_config_defaults(&self, manifest: &WebappManifest) -> StateResult<()> {
    for field in &manifest.config {
      let Some(default) = field.default_as_storage() else {
        continue;
      };
      let key = field.key();
      if self.config_get(manifest.id, key).await?.is_some() {
        continue;
      }
      self.config_set(manifest.id, key, default).await?;
    }
    Ok(())
  }

  pub async fn device_get(&self, key: &str) -> StateResult<Option<String>> {
    self.read_raw(&device_namespace_key(key)).await
  }

  pub async fn device_set(&self, key: &str, value: String) -> StateResult<()> {
    self.write_raw(device_namespace_key(key), value).await
  }

  pub async fn device_delete(&self, key: &str) -> StateResult<()> {
    self.delete_raw(&device_namespace_key(key)).await
  }

  pub async fn webapp_purge(&self, app_id: Uuid) -> StateResult<()> {
    let data_pattern = format!("{}:data:%", app_id.simple());
    let config_pattern = format!("{}:config:%", app_id.simple());
    let doc_pattern = format!("{}:doc:%", app_id.simple());
    let tx = self.db.begin().await?;
    KvEntity::delete_many()
      .filter(KvColumn::Key.like(&data_pattern))
      .exec(&tx)
      .await?;
    KvEntity::delete_many()
      .filter(KvColumn::Key.like(&config_pattern))
      .exec(&tx)
      .await?;
    KvEntity::delete_many()
      .filter(KvColumn::Key.like(&doc_pattern))
      .exec(&tx)
      .await?;
    tx.commit().await?;
    Ok(())
  }
}

fn data_namespace_key(app_id: Uuid, key: &str) -> String {
  format!("{}:data:{key}", app_id.simple())
}

fn device_namespace_key(key: &str) -> String {
  format!("device:{key}")
}

fn config_namespace_key(app_id: Uuid, key: &str) -> String {
  format!("{}:config:{key}", app_id.simple())
}

fn config_namespace_prefix(app_id: Uuid) -> String {
  format!("{}:config:", app_id.simple())
}

fn doc_namespace_key(app_id: Uuid, key: &str) -> String {
  format!("{}:doc:{key}", app_id.simple())
}

fn doc_namespace_prefix(app_id: Uuid) -> String {
  format!("{}:doc:", app_id.simple())
}

fn blob_namespace_key(app_id: Uuid, key: &str) -> String {
  format!("{}:blob:{key}", app_id.simple())
}

fn blob_namespace_prefix(app_id: Uuid) -> String {
  format!("{}:blob:", app_id.simple())
}
