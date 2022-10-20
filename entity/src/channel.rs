//! SeaORM Entity. Generated by sea-orm-codegen 0.9.3

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "channel")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::channel_user::Entity")]
    ChannelUser,
}

impl Related<super::channel_user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ChannelUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
