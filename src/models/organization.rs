use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

pub const DEFAULT_SYSTEM_ROLES: &[(&str, &str, &str, &[&str])] = &[
    (
        "owner",
        "Owner",
        "Pemilik organisasi dengan hak akses penuh tanpa batasan.",
        &[
            "org:view",
            "org:edit",
            "org:delete",
            "org:manage_members",
            "org:manage_roles",
            "event:create",
            "event:edit",
            "event:delete",
            "event:manage_team",
            "finance:view",
            "finance:manage_budget",
            "finance:submit_expense",
            "finance:audit_spj",
            "sponsor:view",
            "sponsor:manage",
            "cert:issue",
            "cert:verify",
        ],
    ),
    (
        "admin",
        "Admin",
        "Administrator organisasi untuk mengelola seluruh aspek tim, event, keuangan, dan sponsorship (kecuali menghapus organisasi).",
        &[
            "org:view",
            "org:edit",
            "org:manage_members",
            "org:manage_roles",
            "event:create",
            "event:edit",
            "event:delete",
            "event:manage_team",
            "finance:view",
            "finance:manage_budget",
            "finance:submit_expense",
            "finance:audit_spj",
            "sponsor:view",
            "sponsor:manage",
            "cert:issue",
            "cert:verify",
        ],
    ),
    (
        "treasurer",
        "Bendahara",
        "Mengelola anggaran RAB, verifikasi audit kuitansi SPJ, dan pencatatan kas organisasi.",
        &[
            "org:view",
            "event:manage_team",
            "finance:view",
            "finance:manage_budget",
            "finance:submit_expense",
            "finance:audit_spj",
            "sponsor:view",
        ],
    ),
    (
        "event_manager",
        "Koordinator Acara",
        "Membuat dan mengelola event, susunan kepanitiaan, tiket, serta memantau pengeluaran acara.",
        &[
            "org:view",
            "event:create",
            "event:edit",
            "event:manage_team",
            "finance:view",
            "finance:submit_expense",
            "sponsor:view",
            "sponsor:manage",
            "cert:issue",
        ],
    ),
    (
        "member",
        "Anggota",
        "Anggota organisasi standar dengan akses melihat agenda, mengajukan reimbursement, dan memvalidasi sertifikat.",
        &[
            "org:view",
            "finance:view",
            "finance:submit_expense",
            "sponsor:view",
            "cert:verify",
        ],
    ),
];

pub static PERMISSIONS_CATALOG: &[(&str, &str, &str, &str)] = &[
    // (key, category, display_name, description)
    ("org:view", "organization", "Lihat Organisasi", "Melihat profil, analitik, dan ringkasan organisasi"),
    ("org:edit", "organization", "Edit Organisasi", "Mengubah profil, identitas, dan pengaturan organisasi"),
    ("org:delete", "organization", "Hapus Organisasi", "Menghapus organisasi dan seluruh entitas di dalamnya"),
    ("org:manage_members", "organization", "Kelola Anggota", "Menambah, mengubah role, dan mengeluarkan anggota"),
    ("org:manage_roles", "organization", "Kelola Role & Matriks", "Membuat role baru dan mengatur matriks hak akses"),
    ("event:create", "events", "Buat Event", "Membuat event baru di bawah organisasi"),
    ("event:edit", "events", "Edit Event", "Mengubah pengaturan, jadwal, dan alokasi tiket event"),
    ("event:delete", "events", "Hapus Event", "Menghapus event dari sistem"),
    ("event:manage_team", "events", "Kelola Tim Event", "Mengatur susunan panitia per-event"),
    ("finance:view", "finances", "Lihat Anggaran & SPJ", "Melihat ringkasan RAB dan realisasi kas"),
    ("finance:manage_budget", "finances", "Ubah Pagu RAB", "Mengubah alokasi nominal kategori RAB"),
    ("finance:submit_expense", "finances", "Ajukan SPJ Kuitansi", "Mengajukan klaim pengeluaran dan upload kuitansi"),
    ("finance:audit_spj", "finances", "Audit & Verifikasi SPJ", "Memverifikasi atau menolak pengajuan SPJ kuitansi"),
    ("sponsor:view", "sponsorship", "Lihat Sponsor", "Melihat proposal dan pipeline sponsorship"),
    ("sponsor:manage", "sponsorship", "Kelola Sponsor", "Mengubah status negosiasi dan paket sponsor"),
    ("cert:issue", "certificates", "Terbitkan Sertifikat", "Menerbitkan e-sertifikat digital peserta & panitia"),
    ("cert:verify", "certificates", "Verifikasi Sertifikat", "Memverifikasi keabsahan kode sertifikat"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionItem {
    pub key: String,
    pub category: String,
    pub display_name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionCategoryGroup {
    pub category: String,
    pub label: String,
    pub permissions: Vec<PermissionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub company: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub member_count: Option<i64>,
    #[serde(default)]
    pub event_count: Option<i64>,
    #[serde(default)]
    pub my_role: Option<String>,
    #[serde(default)]
    pub my_role_display: Option<String>,
    #[serde(default)]
    pub my_permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationRole {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_system: bool,
    pub permissions: sqlx::types::Json<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub member_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationMemberDetail {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub username: String,
    pub email: String,
    pub company: Option<String>,
    pub role_id: Uuid,
    pub role_name: String,
    pub role_display_name: String,
    pub custom_title: Option<String>,
    pub status: String,
    pub joined_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrganizationPayload {
    pub name: String,
    pub slug: Option<String>,
    pub company: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOrganizationPayload {
    pub name: Option<String>,
    pub company: Option<String>,
    pub description: Option<String>,
    pub logo_url: Option<String>,
    pub banner_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRolePayload {
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRolePayload {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMemberPayload {
    pub identifier: String, // email or username
    pub role_id: Uuid,
    pub custom_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemberRolePayload {
    pub role_id: Uuid,
    pub custom_title: Option<String>,
}
