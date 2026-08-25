use sea_orm_migration::prelude::*;

pub fn migrations() -> Vec<Box<dyn MigrationTrait>> {
  vec![
    Box::new(M0002CreateState),
    Box::new(M0003CreateWebappProvenance),
    Box::new(M0004DeviceIdentity),
  ]
}

struct M0002CreateState;

impl MigrationName for M0002CreateState {
  fn name(&self) -> &str {
    "m0002_create_state"
  }
}

#[async_trait::async_trait]
impl MigrationTrait for M0002CreateState {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .create_table(
        Table::create()
          .table(Meta::Table)
          .if_not_exists()
          .col(ColumnDef::new(Meta::Key).text().not_null().primary_key())
          .col(ColumnDef::new(Meta::Value).text().not_null())
          .to_owned(),
      )
      .await?;

    manager
      .create_table(
        Table::create()
          .table(KvStorage::Table)
          .if_not_exists()
          .col(ColumnDef::new(KvStorage::Key).text().not_null().primary_key())
          .col(ColumnDef::new(KvStorage::Value).text().not_null())
          .to_owned(),
      )
      .await?;

    manager
      .create_table(
        Table::create()
          .table(Devices::Table)
          .if_not_exists()
          .col(ColumnDef::new(Devices::Mac).text().not_null().primary_key())
          .col(ColumnDef::new(Devices::Name).text().not_null())
          .col(ColumnDef::new(Devices::DeviceType).text().not_null())
          .col(ColumnDef::new(Devices::IsDefault).boolean().not_null().default(false))
          .to_owned(),
      )
      .await
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .drop_table(Table::drop().table(Devices::Table).to_owned())
      .await?;
    manager
      .drop_table(Table::drop().table(KvStorage::Table).to_owned())
      .await?;
    manager.drop_table(Table::drop().table(Meta::Table).to_owned()).await
  }
}

struct M0003CreateWebappProvenance;

impl MigrationName for M0003CreateWebappProvenance {
  fn name(&self) -> &str {
    "m0003_create_webapp_provenance"
  }
}

#[async_trait::async_trait]
impl MigrationTrait for M0003CreateWebappProvenance {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .create_table(
        Table::create()
          .table(WebappProvenance::Table)
          .if_not_exists()
          .col(
            ColumnDef::new(WebappProvenance::WebappId)
              .text()
              .not_null()
              .primary_key(),
          )
          .col(ColumnDef::new(WebappProvenance::Provenance).text().not_null())
          .to_owned(),
      )
      .await
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .drop_table(Table::drop().table(WebappProvenance::Table).to_owned())
      .await
  }
}

struct M0004DeviceIdentity;

impl MigrationName for M0004DeviceIdentity {
  fn name(&self) -> &str {
    "m0004_device_identity"
  }
}

#[async_trait::async_trait]
impl MigrationTrait for M0004DeviceIdentity {
  async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .alter_table(
        Table::alter()
          .table(Devices::Table)
          .rename_column(Devices::Mac, Alias::new("id"))
          .to_owned(),
      )
      .await?;
    manager
      .alter_table(
        Table::alter()
          .table(Devices::Table)
          .add_column(
            ColumnDef::new(Alias::new("kind"))
              .text()
              .not_null()
              .default("bluetooth"),
          )
          .to_owned(),
      )
      .await
  }

  async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
    manager
      .alter_table(
        Table::alter()
          .table(Devices::Table)
          .drop_column(Alias::new("kind"))
          .to_owned(),
      )
      .await?;
    manager
      .alter_table(
        Table::alter()
          .table(Devices::Table)
          .rename_column(Alias::new("id"), Devices::Mac)
          .to_owned(),
      )
      .await
  }
}

#[derive(DeriveIden)]
enum Meta {
  Table,
  Key,
  Value,
}

#[derive(DeriveIden)]
enum WebappProvenance {
  Table,
  WebappId,
  Provenance,
}

#[derive(DeriveIden)]
enum KvStorage {
  Table,
  Key,
  Value,
}

#[derive(DeriveIden)]
enum Devices {
  Table,
  Mac,
  Name,
  DeviceType,
  IsDefault,
}
