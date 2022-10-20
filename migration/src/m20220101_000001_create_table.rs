use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;


#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {

        manager
            .create_table(
                Table::create()
                    .table(Channel::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Channel::Name)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .to_owned(),
            )
            .await?;
        // manager.create_index(Index::create()
        //     .name("channel_name_idx")
        //     .table(Channel::Table)
        //     .col(Channel::Name)
        //     .to_owned()).await?;
        manager
            .create_table(
                Table::create()
                    .table(User::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(User::Name)
                            .string()
                            .not_null()
                            // .auto_increment()
                            .primary_key(),
                    )
                    // .col(ColumnDef::new(User::Name).string().not_null())
                    .to_owned(),
            )
            .await?;

        // manager.create_index(Index::create()
        //     .name("user_name_idx")
        //     .table(User::Table)
        //     .col(User::Name)
        //     .to_owned()).await?;
        manager
            .create_table(
                Table::create()
                    .table(ChannelUser::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(ChannelUser::ChannelId).string().not_null())
                    .col(ColumnDef::new(ChannelUser::UserId).string().not_null())
                    .col(ColumnDef::new(ChannelUser::Pyramids).integer().not_null())
                    .primary_key(&mut Index::create()
                        .name("merge_primary")
                        .table(ChannelUser::Table)
                        .col(ChannelUser::ChannelId)
                        .col(ChannelUser::UserId)
                        .primary()
                        .to_owned())
                    .foreign_key(&mut ForeignKey::create()
                        .name("FK_channel_id")
                        .from(ChannelUser::Table, ChannelUser::ChannelId)
                        .to(Channel::Table, Channel::Name)
                        .on_delete(ForeignKeyAction::Cascade)
                        .on_update(ForeignKeyAction::Cascade)
                        .to_owned())
                    .foreign_key(&mut ForeignKey::create()
                        .name("FK_user_id")
                        .from(ChannelUser::Table, ChannelUser::UserId)
                        .to(User::Table, User::Name)
                        .on_delete(ForeignKeyAction::Cascade)
                        .on_update(ForeignKeyAction::Cascade)
                        .to_owned())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {

        manager
            .drop_table(Table::drop().table(ChannelUser::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Channel::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(User::Table).to_owned())
            .await

    }
}

/// Learn more at https://docs.rs/sea-query#iden
#[derive(Iden)]
enum Channel {
    Table,
    Name,
}

#[derive(Iden)]
enum User {
    Table,
    Name,
}

#[derive(Iden)]
enum ChannelUser {
    Table,
    ChannelId,
    UserId,
    Pyramids
}
