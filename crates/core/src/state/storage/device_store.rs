use std::collections::HashMap;

use libbridgething::{Device, LinkKind};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, TransactionTrait};

use super::{
  super::{StateError, StateResult},
  device::{Column as DeviceColumn, Entity as DeviceEntity, Model as DeviceModel, link_kind_str},
  meta::{Column as MetaColumn, Entity as MetaEntity, KEY_LAST_DEVICE},
};
use crate::{net::WireEventBus, stock::StockSetupSend};

#[derive(Debug, Clone)]
pub struct DeviceStore {
  db: DatabaseConnection,
  bus: WireEventBus,
}

impl DeviceStore {
  pub fn new(db: DatabaseConnection, bus: WireEventBus) -> Self {
    Self { db, bus }
  }

  pub async fn remember(&self, device: &Device) -> StateResult<bool> {
    let fresh = self.get(&device.id).await?.is_none();
    self.upsert(device.clone()).await?;
    self.set_last(device.id.clone()).await?;
    if fresh && let Err(errs) = self.bus.broadcast_stock(StockSetupSend::finished()).await {
      tracing::debug!(count = errs.len(), "stock setup completion broadcast errors");
    }
    Ok(fresh)
  }

  pub async fn list(&self, kind: LinkKind) -> StateResult<HashMap<String, Device>> {
    let rows = DeviceEntity::find()
      .filter(DeviceColumn::Kind.eq(link_kind_str(kind)))
      .all(&self.db)
      .await
      .map_err(StateError::from)?;
    Ok(rows.iter().map(|m| (m.id.clone(), Device::from(m))).collect())
  }

  pub async fn get(&self, id: &str) -> StateResult<Option<Device>> {
    Ok(
      DeviceEntity::find_by_id(id.to_string())
        .one(&self.db)
        .await?
        .as_ref()
        .map(Device::from),
    )
  }

  pub async fn upsert(&self, device: Device) -> StateResult<()> {
    let model = DeviceModel::from_wire(&device).into_active_model();
    DeviceEntity::insert(model)
      .on_conflict(
        sea_orm::sea_query::OnConflict::column(DeviceColumn::Id)
          .update_columns([
            DeviceColumn::Name,
            DeviceColumn::DeviceType,
            DeviceColumn::Kind,
            DeviceColumn::IsDefault,
          ])
          .to_owned(),
      )
      .exec(&self.db)
      .await?;
    Ok(())
  }

  pub async fn remove(&self, id: String) -> StateResult<()> {
    let tx = self.db.begin().await?;
    DeviceEntity::delete_by_id(id.clone()).exec(&tx).await?;
    let last = MetaEntity::find_by_id(KEY_LAST_DEVICE.to_string()).one(&tx).await?;
    if last.map(|m| m.value) == Some(id) {
      MetaEntity::delete_by_id(KEY_LAST_DEVICE.to_string()).exec(&tx).await?;
    }
    tx.commit().await?;
    Ok(())
  }

  pub async fn last(&self) -> StateResult<Option<String>> {
    Ok(
      MetaEntity::find_by_id(KEY_LAST_DEVICE.to_string())
        .one(&self.db)
        .await?
        .map(|m| m.value),
    )
  }

  pub async fn set_last(&self, id: String) -> StateResult<()> {
    let model = super::meta::ActiveModel {
      key: sea_orm::Set(KEY_LAST_DEVICE.to_string()),
      value: sea_orm::Set(id),
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
