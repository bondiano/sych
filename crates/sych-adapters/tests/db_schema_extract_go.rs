//! Regression tests for Go static ORM extractors (GORM, Bun, Ent):
//! cross-package resolution, embedded prefixes, field-order independence,
//! multi-return `TableName()`, multiline Ent chains and orphan M2M guards.

use std::fs;

use sych_adapters::db_schema::extract::{SourceTree, extract_models};
use sych_core::db_schema::{DbModelManifest, ModelExtractionSettings, ModelExtractorKind};

fn extract_go(kind: ModelExtractorKind, files: &[(&str, &str)]) -> DbModelManifest {
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
fn gorm_edge_cases_field_order_embedded_cross_package_prefilter_and_m2m() {
    let manifest = extract_go(
        ModelExtractorKind::Gorm,
        &[
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
        ],
    );

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
    assert_eq!(member.relations.len(), 1);
    assert_eq!(member.relations[0].columns, ["profile_id"]);
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
    assert!(!member.columns.iter().any(|c| c.name == "id"));
    assert!(!member.columns.iter().any(|c| c.name == "skip_mig"));
    assert!(!manifest.tables.iter().any(|t| t.name == "orphan_tags"));
}

#[test]
fn gorm_prefixed_embed_relation_multi_return_tablename_and_order_independent_upsert() {
    let manifest = extract_go(
        ModelExtractorKind::Gorm,
        &[(
            "app/models.go",
            r#"
package app
import ("gorm.io/gorm"; "time")
type Author struct {
    ID uint `gorm:"primaryKey"`
}
type Audit struct {
    Author   Author
    AuthorID uint `gorm:"not null"`
}
type Article struct {
    ID   uint  `gorm:"primaryKey"`
    Meta Audit `gorm:"embedded;embeddedPrefix:b_"`
}
type DynamicAccount struct {
    ID   uint   `gorm:"primaryKey"`
    Name string `gorm:"not null"`
}
func (d DynamicAccount) TableName() string {
    if d.Name == "admin" {
        return "admins"
    }
    return "accounts"
}
type OrderA struct {
    gorm.Model
    CreatedAt time.Time `gorm:"unique;size:32"`
}
type OrderB struct {
    CreatedAt time.Time `gorm:"unique;size:32"`
    gorm.Model
}
"#,
        )],
    );

    let article = manifest
        .tables
        .iter()
        .find(|t| t.name == "articles")
        .expect("articles");
    assert_eq!(article.relations.len(), 1);
    assert_eq!(article.relations[0].columns, ["b_author_id"]);
    assert_eq!(article.relations[0].references.table, "authors");

    // Multi-return TableName() falls back to default table name instead of "admins".
    assert!(!manifest.tables.iter().any(|t| t.name == "admins"));
    assert!(manifest.tables.iter().any(|t| t.name == "dynamic_accounts"));

    // Duplicate column merge is order-independent across field declarations.
    let ca_a = manifest
        .tables
        .iter()
        .find(|t| t.name == "order_as")
        .expect("order_as")
        .columns
        .iter()
        .find(|c| c.name == "created_at")
        .expect("created_at A");
    let ca_b = manifest
        .tables
        .iter()
        .find(|t| t.name == "order_bs")
        .expect("order_bs")
        .columns
        .iter()
        .find(|c| c.name == "created_at")
        .expect("created_at B");
    assert_eq!(ca_a.required, ca_b.required);
    assert!(ca_a.required);
    assert_eq!(ca_a.unique, ca_b.unique);
    assert!(ca_a.unique);
    assert_eq!(ca_a.max_length, ca_b.max_length);
    assert_eq!(ca_a.max_length, Some(32));
}

#[test]
fn bun_cross_package_models_and_m2m_only_guard() {
    let manifest = extract_go(
        ModelExtractorKind::Bun,
        &[
            (
                "pkg_a/models.go",
                r#"
package pkg_a
import "github.com/uptrace/bun"
type User struct {
    bun.BaseModel `bun:"table:a_users"`
    ID int64 `bun:",pk"`
}
type Order struct {
    bun.BaseModel `bun:"table:a_orders"`
    ID     int64 `bun:",pk"`
    UserID int64 `bun:",notnull"`
    User   *User `bun:"rel:belongs-to,join:user_id=id"`
}
type OrphanM2M struct {
    bun.BaseModel `bun:"table:orphan_owners"`
    Users []User `bun:"m2m:orphan_user_links"`
}
"#,
            ),
            (
                "pkg_b/models.go",
                r#"
package pkg_b
import "github.com/uptrace/bun"
type User struct {
    bun.BaseModel `bun:"table:b_users"`
    ID int64 `bun:",pk"`
}
type Invoice struct {
    bun.BaseModel `bun:"table:b_invoices"`
    ID     int64 `bun:",pk"`
    UserID int64 `bun:",notnull"`
    User   *User `bun:"rel:belongs-to,join:user_id=id"`
}
"#,
            ),
        ],
    );

    let a_order = manifest
        .tables
        .iter()
        .find(|t| t.name == "a_orders")
        .expect("a_orders");
    assert_eq!(a_order.relations[0].references.table, "a_users");
    let b_inv = manifest
        .tables
        .iter()
        .find(|t| t.name == "b_invoices")
        .expect("b_invoices");
    assert_eq!(b_inv.relations[0].references.table, "b_users");
    assert!(
        !manifest
            .tables
            .iter()
            .any(|t| t.name == "orphan_user_links")
    );
}

#[test]
fn ent_edge_order_multiline_indexes_and_multi_package_schemas() {
    let manifest = extract_go(
        ModelExtractorKind::Ent,
        &[
            (
                "svc_a/ent/schema/user.go",
                r#"
package schema
import ("entgo.io/ent"; "entgo.io/ent/schema/edge"; "entgo.io/ent/schema/field"; "entgo.io/ent/schema/index")
type User struct { ent.Schema }
func (User) Annotations() []schema.Annotation { return []schema.Annotation{entsql.Annotation{Table: "svc_a_users"}} }
func (User) Fields() []ent.Field {
    return []ent.Field{
        field.String("email").
            Unique().
            MaxLen(120),
        field.Int("org_id"),
        field.Int("team_id"),
    }
}
func (User) Indexes() []ent.Index {
    return []ent.Index{
        index.Fields("org_id", "email").
            Unique(),
    }
}
func (User) Edges() []ent.Edge {
    return []ent.Edge{
        edge.To("team", Team.Type).
            Field("team_id"),
        edge.From(
            "org",
            Org.Type,
        ).
            Field("org_id").
            Unique(),
    }
}
type Team struct { ent.Schema }
type Org struct { ent.Schema }
"#,
            ),
            (
                "svc_b/ent/schema/user.go",
                r#"
package schema
import ("entgo.io/ent"; "entgo.io/ent/schema/field")
type User struct { ent.Schema }
func (User) Annotations() []schema.Annotation { return []schema.Annotation{entsql.Annotation{Table: "svc_b_users"}} }
func (User) Fields() []ent.Field { return []ent.Field{field.String("handle")} }
"#,
            ),
        ],
    );

    let a_user = manifest
        .tables
        .iter()
        .find(|t| t.name == "svc_a_users")
        .expect("svc_a_users");
    assert!(
        a_user
            .columns
            .iter()
            .any(|c| c.name == "email" && c.unique && c.max_length == Some(120))
    );
    assert_eq!(a_user.uniques.len(), 1);
    assert_eq!(a_user.uniques[0].columns, ["org_id", "email"]);
    assert_eq!(a_user.relations.len(), 2);
    assert!(
        a_user
            .relations
            .iter()
            .any(|r| r.columns == ["team_id"] && r.references.table == "teams")
    );
    assert!(
        a_user
            .relations
            .iter()
            .any(|r| r.columns == ["org_id"] && r.references.table == "orgs")
    );
    assert!(manifest.tables.iter().any(|t| t.name == "svc_b_users"));
}

#[test]
fn ent_ambiguous_edge_target_records_limitation_instead_of_guessing() {
    let manifest = extract_go(
        ModelExtractorKind::Ent,
        &[
            (
                "svc_a/ent/schema/user.go",
                "package schema\nimport \"entgo.io/ent\"\ntype User struct { ent.Schema }\nfunc (User) Annotations() []schema.Annotation { return []schema.Annotation{entsql.Annotation{Table: \"svc_a_users\"}} }\n",
            ),
            (
                "svc_b/ent/schema/user.go",
                "package schema\nimport \"entgo.io/ent\"\ntype User struct { ent.Schema }\nfunc (User) Annotations() []schema.Annotation { return []schema.Annotation{entsql.Annotation{Table: \"svc_b_users\"}} }\n",
            ),
            (
                "svc_c/ent/schema/post.go",
                r#"
package schema
import ("entgo.io/ent"; "entgo.io/ent/schema/edge"; "entgo.io/ent/schema/field")
type Post struct { ent.Schema }
func (Post) Fields() []ent.Field { return []ent.Field{field.Int("user_id")} }
func (Post) Edges() []ent.Edge { return []ent.Edge{edge.To("user", User.Type).Field("user_id")} }
"#,
            ),
        ],
    );

    let post = manifest
        .tables
        .iter()
        .find(|t| t.name == "posts")
        .expect("posts");
    assert!(post.relations.is_empty(), "{:?}", post.relations);
    assert!(
        manifest
            .limitations
            .iter()
            .any(|l| l.contains("ambiguous edge target `User`")),
        "{:?}",
        manifest.limitations
    );
}

#[test]
fn gorm_has_one_does_not_emit_phantom_fk_and_custom_fk_column_resolves() {
    let manifest = extract_go(
        ModelExtractorKind::Gorm,
        &[(
            "app/models.go",
            r#"
package app
type Card struct {
    ID     uint `gorm:"primaryKey"`
    UserID uint `gorm:"not null"`
}
type Customer struct {
    ID   uint `gorm:"primaryKey"`
    Card Card `gorm:"foreignKey:UserID"`
}
type Invoice struct {
    ID      uint     `gorm:"primaryKey"`
    OwnerID uint     `gorm:"column:buyer_id;not null"`
    Owner   Customer `gorm:"foreignKey:OwnerID"`
}
"#,
        )],
    );

    let customer = manifest
        .tables
        .iter()
        .find(|t| t.name == "customers")
        .expect("customers");
    // Has-one FK (`UserID`) lives on `cards`, not `customers`.
    assert!(customer.relations.is_empty(), "{:?}", customer.relations);

    let invoice = manifest
        .tables
        .iter()
        .find(|t| t.name == "invoices")
        .expect("invoices");
    assert_eq!(invoice.relations.len(), 1);
    assert_eq!(invoice.relations[0].columns, ["buyer_id"]);
    assert_eq!(invoice.relations[0].references.table, "customers");
}

#[test]
fn ent_id_storage_key_bun_aliased_imports_and_gorm_serialized_slices() {
    // Ent custom StorageKey on id updates primary_key and ref_columns.
    let ent = extract_go(
        ModelExtractorKind::Ent,
        &[(
            "ent/schema/account.go",
            r#"
package schema
import ("entgo.io/ent"; "entgo.io/ent/schema/edge"; "entgo.io/ent/schema/field")
type Account struct { ent.Schema }
func (Account) Fields() []ent.Field {
    return []ent.Field{field.Int("id").StorageKey("account_uid")}
}
type Token struct { ent.Schema }
func (Token) Fields() []ent.Field { return []ent.Field{field.Int("account_uid")} }
func (Token) Edges() []ent.Edge {
    return []ent.Edge{edge.From("account", Account.Type).Field("account_uid").Unique()}
}
"#,
        )],
    );
    let acc = ent
        .tables
        .iter()
        .find(|t| t.name == "accounts")
        .expect("accounts");
    assert_eq!(acc.primary_key, ["account_uid"]);
    let tok = ent
        .tables
        .iter()
        .find(|t| t.name == "tokens")
        .expect("tokens");
    assert_eq!(tok.relations[0].ref_columns, ["account_uid"]);

    // Bun aliased uptrace/bun embed (with and without tag) + aliased package import relation.
    let bun = extract_go(
        ModelExtractorKind::Bun,
        &[
            (
                "pkg_models/user.go",
                "package pkg_models\nimport b \"github.com/uptrace/bun\"\ntype User struct {\n    b.BaseModel `bun:\"table:custom_users\"`\n    ID int64\n}\ntype Minimal struct {\n    b.BaseModel\n    ID int64\n}\n",
            ),
            (
                "pkg_app/post.go",
                "package pkg_app\nimport (\n    b \"github.com/uptrace/bun\"\n    m \"example.com/pkg_models\"\n)\ntype Post struct {\n    b.BaseModel `bun:\"table:posts\"`\n    ID       int64   `bun:\",pk\"`\n    AuthorID int64   `bun:\",notnull\"`\n    Author   *m.User `bun:\"rel:belongs-to,join:author_id=id\"`\n}\n",
            ),
        ],
    );
    assert!(bun.tables.iter().any(|t| t.name == "custom_users"));
    assert!(bun.tables.iter().any(|t| t.name == "minimals"));
    let post = bun
        .tables
        .iter()
        .find(|t| t.name == "posts")
        .expect("posts");
    assert_eq!(post.relations.len(), 1);
    assert_eq!(post.relations[0].references.table, "custom_users");

    // GORM non-[]byte slices with serializer/type tag are kept as scalar columns.
    let gorm = extract_go(
        ModelExtractorKind::Gorm,
        &[(
            "app/post.go",
            r#"
package app
type Note struct {
    ID     uint     `gorm:"primaryKey"`
    Tags   []string `gorm:"serializer:json;not null"`
    Labels []string `gorm:"type:text[]"`
    Other  []int
}
"#,
        )],
    );
    let note = gorm
        .tables
        .iter()
        .find(|t| t.name == "notes")
        .expect("notes");
    assert!(note.columns.iter().any(|c| c.name == "tags" && c.required));
    assert!(note.columns.iter().any(|c| c.name == "labels"));
    assert!(!note.columns.iter().any(|c| c.name == "other"));
}
