//! Integration tests for all 9 built-in static ORM model extractors
//! (Go GORM/Bun/Ent, Prisma, TS Drizzle, Python SQLAlchemy/Django, Rust Diesel/SeaORM).

use std::collections::BTreeMap;
use std::fs;

use sych_adapters::db_schema::extract::naming::default_table_name;
use sych_adapters::db_schema::extract::{SourceTree, extract_models};
use sych_core::db_schema::{
    DbModelManifest, ModelExtractionSettings, ModelExtractorKind, ModelNamingOverrides,
};

fn extract_from_files(kind: ModelExtractorKind, files: &[(&str, &str)]) -> DbModelManifest {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, content) in files {
        let full = dir.path().join(rel);
        if let Some(p) = full.parent() {
            fs::create_dir_all(p).expect("mkdir");
        }

        fs::write(&full, content).expect("write");
    }
    let tree = SourceTree::from_dir(dir.path());
    let settings = ModelExtractionSettings {
        extractors: vec![kind],
        ..ModelExtractionSettings::default()
    };
    extract_models(&tree, &settings)
}

#[test]
fn orm_extractors_go_gorm_and_bun() {
    // 1. Go GORM
    let gorm = extract_from_files(
        ModelExtractorKind::Gorm,
        &[(
            "models/models.go",
            r#"
package models
import "gorm.io/gorm"
type User struct {
    gorm.Model
    Email string `gorm:"not null;unique;size:255" validate:"required,max=255"`
    Role  string `gorm:"not null" validate:"oneof=admin member"`
    Tags  []Tag  `gorm:"many2many:user_tags"`
}
type Tag struct {
    ID uint `gorm:"primaryKey"`
}
type Order struct {
    ID     uint `gorm:"primaryKey"`
    UserID uint `gorm:"not null"`
    User   User `gorm:"foreignKey:UserID"`
}
"#,
        )],
    );
    assert!(gorm.tables.iter().any(|t| {
        t.name == "users"
            && t.columns
                .iter()
                .any(|c| c.name == "email" && c.unique && c.max_length == Some(255))
    }));
    assert!(
        gorm.tables
            .iter()
            .any(|t| t.name == "orders" && !t.relations.is_empty())
    );
    assert!(
        gorm.tables
            .iter()
            .any(|t| t.name == "user_tags" && t.join_table_for.len() == 2)
    );

    // 2. Go Bun
    let bun = extract_from_files(
        ModelExtractorKind::Bun,
        &[(
            "models/bun.go",
            r#"
package models
import "github.com/uptrace/bun"
type Account struct {
    bun.BaseModel `bun:"table:accounts,alias:a"`
    ID    int64  `bun:",pk"`
    Email string `bun:"email,notnull,unique,type:varchar(180)"`
}
type Invoice struct {
    bun.BaseModel `bun:"table:invoices"`
    ID        int64    `bun:",pk"`
    AccountID int64    `bun:",notnull"`
    Account   *Account `bun:"rel:belongs-to,join:account_id=id"`
}
"#,
        )],
    );
    assert!(bun.tables.iter().any(|t| {
        t.name == "accounts"
            && t.columns
                .iter()
                .any(|c| c.name == "email" && c.required && c.unique && c.max_length == Some(180))
    }));
    assert!(
        bun.tables
            .iter()
            .any(|t| t.name == "invoices" && t.relations.len() == 1)
    );
}

#[test]
fn orm_extractor_go_ent() {
    let ent = extract_from_files(
        ModelExtractorKind::Ent,
        &[(
            "ent/schema/user.go",
            r#"
package schema
import ("entgo.io/ent"; "entgo.io/ent/schema/field"; "entgo.io/ent/schema/index")
type User struct { ent.Schema }
func (User) Fields() []ent.Field {
    return []ent.Field{
        field.String("email").Unique().MaxLen(255),
        field.String("bio").Optional(),
        field.Enum("status").Values("active", "disabled"),
    }
}
func (User) Indexes() []ent.Index {
    return []ent.Index{
        index.Fields("email", "status").Unique(),
    }
}
"#,
        )],
    );
    let u = ent
        .tables
        .iter()
        .find(|t| t.name == "users")
        .expect("ent users");
    assert!(
        u.columns
            .iter()
            .any(|c| c.name == "email" && c.required && c.unique && c.max_length == Some(255))
    );
    assert!(u.columns.iter().any(|c| c.name == "bio" && !c.required));
    assert!(
        u.columns
            .iter()
            .any(|c| c.name == "status" && c.enum_values == ["active", "disabled"])
    );
    assert_eq!(u.uniques.len(), 1);
}

#[test]
fn orm_extractors_prisma_and_drizzle() {
    // 4. Prisma
    let prisma = extract_from_files(
        ModelExtractorKind::Prisma,
        &[(
            "prisma/schema.prisma",
            r#"
enum Role {
  ADMIN @map("admin")
  MEMBER @map("member")
}
model User {
  id    Int     @id
  email String  @unique @db.VarChar(200)
  role  Role
  bio   String?
  @@map("users")
}
model Post {
  id       Int  @id
  authorId Int  @map("author_id")
  author   User @relation(fields: [authorId], references: [id])
  @@map("posts")
}
"#,
        )],
    );
    assert!(prisma.tables.iter().any(|t| {
        t.name == "users"
            && t.columns
                .iter()
                .any(|c| c.name == "role" && c.enum_values == ["admin", "member"])
    }));
    assert!(prisma.tables.iter().any(|t| t.name == "posts"
        && t.relations.len() == 1
        && t.relations[0].columns == ["author_id"]));

    // 5. TypeScript Drizzle
    let drizzle = extract_from_files(
        ModelExtractorKind::Drizzle,
        &[(
            "src/db/schema.ts",
            r"
import { pgTable, pgEnum, serial, varchar, integer, uniqueIndex } from 'drizzle-orm/pg-core';
export const statusEnum = pgEnum('status', ['open', 'closed']);
export const users = pgTable('users', {
    id: serial('id').primaryKey(),
    email: varchar('email', { length: 160 }).notNull().unique(),
    status: statusEnum('status').notNull(),
});
export const tickets = pgTable('tickets', {
    id: serial('id').primaryKey(),
    userId: integer('user_id').notNull().references(() => users.id),
    code: varchar('code', { length: 32 }).notNull(),
}, (t) => [uniqueIndex('uq_tickets_user_code').on(t.userId, t.code)]);
",
        )],
    );
    assert!(drizzle.tables.iter().any(|t| {
        t.name == "users"
            && t.columns
                .iter()
                .any(|c| c.name == "email" && c.max_length == Some(160) && c.unique)
    }));
    assert!(
        drizzle
            .tables
            .iter()
            .any(|t| t.name == "tickets" && t.relations.len() == 1 && t.uniques.len() == 1)
    );
}

#[test]
fn orm_extractors_python_sqlalchemy_and_django() {
    // 6. Python SQLAlchemy
    let sa = extract_from_files(
        ModelExtractorKind::Sqlalchemy,
        &[(
            "app/models.py",
            r#"
import enum
from typing import Optional
from sqlalchemy import Column, ForeignKey, Integer, String, Enum, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

class Plan(enum.Enum):
    FREE = "free"
    PRO = "pro"

class Account(Base):
    __tablename__ = "accounts"
    __table_args__ = (UniqueConstraint("org_id", "slug"),)
    id: Mapped[int] = mapped_column(primary_key=True)
    org_id: Mapped[int] = mapped_column(ForeignKey("orgs.id"), nullable=False)
    slug: Mapped[str] = mapped_column(String(80), unique=True)
    plan: Mapped[str] = mapped_column(Enum(Plan), nullable=False)
    notes: Mapped[Optional[str]] = mapped_column(String(500))
"#,
        )],
    );
    let acc = sa
        .tables
        .iter()
        .find(|t| t.name == "accounts")
        .expect("sa accounts");
    assert!(
        acc.columns
            .iter()
            .any(|c| c.name == "slug" && c.required && c.unique && c.max_length == Some(80))
    );
    assert!(acc.columns.iter().any(|c| c.name == "notes" && !c.required));
    assert!(
        acc.columns
            .iter()
            .any(|c| c.name == "plan" && c.enum_values == ["free", "pro"])
    );
    assert_eq!(acc.relations.len(), 1);
    assert_eq!(acc.uniques.len(), 1);

    // 7. Python Django
    let django = extract_from_files(
        ModelExtractorKind::Django,
        &[(
            "shop/models.py",
            r#"
from django.db import models

class OrderStatus(models.TextChoices):
    PENDING = "pending", "Pending"
    PAID = "paid", "Paid"

class Customer(models.Model):
    email = models.CharField(max_length=254, unique=True)
    class Meta:
        db_table = "customers"

class Order(models.Model):
    customer = models.ForeignKey(Customer, on_delete=models.CASCADE)
    status = models.CharField(max_length=20, choices=OrderStatus.choices)
    tags = models.ManyToManyField("Tag")
    class Meta:
        db_table = "orders"
"#,
        )],
    );
    assert!(django.tables.iter().any(|t| {
        t.name == "customers"
            && t.columns
                .iter()
                .any(|c| c.name == "email" && c.unique && c.max_length == Some(254))
    }));
    assert!(django.tables.iter().any(|t| {
        t.name == "orders"
            && t.relations.len() == 1
            && t.columns
                .iter()
                .any(|c| c.name == "status" && c.enum_values == ["pending", "paid"])
    }));
    assert!(
        django
            .tables
            .iter()
            .any(|t| t.name == "orders_tags" && t.join_table_for.len() == 2)
    );
}

#[test]
fn orm_extractors_rust_diesel_and_seaorm() {
    // 8. Rust Diesel
    let diesel = extract_from_files(
        ModelExtractorKind::Diesel,
        &[(
            "src/schema.rs",
            r"
diesel::table! {
    users (id) {
        id -> Int8,
        #[max_length = 255]
        email -> Varchar,
        bio -> Nullable<Text>,
    }
}
diesel::table! {
    posts (id) {
        id -> Int8,
        user_id -> Int8,
    }
}
diesel::joinable!(posts -> users (user_id));
",
        )],
    );
    let du = diesel
        .tables
        .iter()
        .find(|t| t.name == "users")
        .expect("diesel users");
    assert!(
        du.columns
            .iter()
            .any(|c| c.name == "email" && c.required && c.max_length == Some(255))
    );
    assert!(du.columns.iter().any(|c| c.name == "bio" && !c.required));
    let dp = diesel
        .tables
        .iter()
        .find(|t| t.name == "posts")
        .expect("diesel posts");
    assert_eq!(dp.relations.len(), 1);

    // 9. Rust SeaORM
    let sea = extract_from_files(
        ModelExtractorKind::Seaorm,
        &[(
            "src/entities/order.rs",
            r#"
#[derive(DeriveActiveEnum)]
pub enum Kind {
    #[sea_orm(string_value = "std")]
    Standard,
    #[sea_orm(string_value = "exp")]
    Express,
}

#[derive(DeriveEntityModel)]
#[sea_orm(table_name = "orders")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique)]
    pub code: String,
    pub user_id: i64,
    pub kind: Kind,
    pub notes: Option<String>,
}

#[derive(EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::user::Entity", from = "Column::UserId", to = "super::user::Column::Id")]
    User,
}
"#,
        )],
    );
    let so = sea
        .tables
        .iter()
        .find(|t| t.name == "orders")
        .expect("seaorm orders");
    assert!(
        so.columns
            .iter()
            .any(|c| c.name == "code" && c.required && c.unique)
    );
    assert!(so.columns.iter().any(|c| c.name == "notes" && !c.required));
    assert!(
        so.columns
            .iter()
            .any(|c| c.name == "kind" && c.enum_values == ["std", "exp"])
    );
    assert_eq!(so.relations.len(), 1);
}

#[test]
fn naming_no_lower_case_preserves_case_on_plural_tables() {
    let overrides = ModelNamingOverrides {
        no_lower_case: true,
        singular_table: false,
        table_prefix: String::new(),
    };
    let empty = BTreeMap::new();
    assert_eq!(
        default_table_name("UserProfile", None, &overrides, &empty),
        "UserProfiles"
    );
    assert_eq!(
        default_table_name("UserCategory", None, &overrides, &empty),
        "UserCategories"
    );
    assert_eq!(
        default_table_name("UserStatus", None, &overrides, &empty),
        "UserStatuses"
    );
    assert_eq!(
        default_table_name("Person", None, &overrides, &empty),
        "People"
    );
    assert_eq!(
        default_table_name("Metadata", None, &overrides, &empty),
        "Metadata"
    );
}
