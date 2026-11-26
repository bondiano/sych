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

#[test]
fn gorm_edge_cases_field_order_embedded_cross_package_prefilter_and_m2m() {
    let dir = tempfile::tempdir().expect("tempdir");
    let files = [
        // Struct in a file with no "gorm" substring; TableName() lives in another file.
        (
            "pkg_a/account.go",
            "package pkg_a\ntype Shared struct {\n    ID uint\n    Name string\n}\n",
        ),
        (
            "pkg_a/table_names.go",
            "package pkg_a\nfunc (Shared) TableName() string { return \"pkg_a_shared\" }\n",
        ),
        // Same struct name in a second package with a different table name.
        (
            "pkg_b/account.go",
            "package pkg_b\nimport \"gorm.io/gorm\"\ntype Shared struct {\n    gorm.Model\n    ID uint `gorm:\"primaryKey\"`\n    Email string `gorm:\"not null\"`\n}\nfunc (Shared) TableName() string { return \"pkg_b_shared\" }\n",
        ),
        // Relation declared BEFORE its FK column, named gorm:"embedded" with prefix,
        // non-gorm audit.Model embed, and M2M-only struct whose owner is dropped.
        (
            "app/models.go",
            r#"
package app
import "gorm.io/gorm"
type Address struct {
    City string `gorm:"not null"`
}
type Profile struct {
    ID uint `gorm:"primaryKey"`
}
type Member struct {
    audit.Model
    Profile   Profile
    ProfileID uint    `gorm:"not null"`
    Billing   Address `gorm:"embedded;embeddedPrefix:billing_"`
    SkipMig   string  `gorm:"-:migration"`
}
type OrphanM2M struct {
    Tags []Profile `gorm:"many2many:orphan_tags"`
}
"#,
        ),
    ];
    for (rel, content) in files {
        let full = dir.path().join(rel);
        fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        fs::write(&full, content).expect("write");
    }
    let tree = SourceTree::from_dir(dir.path());
    let settings = ModelExtractionSettings {
        extractors: vec![ModelExtractorKind::Gorm],
        ..ModelExtractionSettings::default()
    };
    let manifest = extract_models(&tree, &settings);

    assert!(manifest.tables.iter().any(|t| t.name == "pkg_a_shared"));
    let pb = manifest
        .tables
        .iter()
        .find(|t| t.name == "pkg_b_shared")
        .expect("pkg_b_shared");
    // Explicit ID deduplicates gorm.Model's ID, while gorm.Model still emits timestamps.
    assert_eq!(pb.columns.iter().filter(|c| c.name == "id").count(), 1);
    assert!(
        pb.columns
            .iter()
            .any(|c| c.name == "created_at" && c.required)
    );
    assert!(
        pb.columns
            .iter()
            .any(|c| c.name == "deleted_at" && !c.required)
    );

    let member = manifest
        .tables
        .iter()
        .find(|t| t.name == "members")
        .expect("members");
    // Relation declared before ProfileID still resolves cleanly.
    assert_eq!(member.relations.len(), 1);
    assert_eq!(member.relations[0].columns, ["profile_id"]);
    // Named gorm:"embedded" flattens columns with prefix and does not create a relation.
    assert!(
        member
            .columns
            .iter()
            .any(|c| c.name == "billing_city" && c.required)
    );
    assert!(
        !member
            .relations
            .iter()
            .any(|r| r.references.table == "addresses")
    );
    // Non-gorm audit.Model does NOT add a spurious id column.
    assert!(!member.columns.iter().any(|c| c.name == "id"));
    // -:migration is skipped.
    assert!(!member.columns.iter().any(|c| c.name == "skip_mig"));
    // M2M-only struct with dropped owning table does not emit a dangling join table.
    assert!(!manifest.tables.iter().any(|t| t.name == "orphan_tags"));
}
