//! The module registry (ADR-0011, R0.1): the single, compile-time-closed list of Pindorama
//! modules and their declarative [`ModuleManifest`]s. This is the source of truth for the R4
//! permission matrix, the per-org module gate (R0.5), and nav composition (R0.7).
//!
//! NÃO é auto-registro por linker (`inventory`/`linkme`): num workspace fechado a lista é
//! conhecida aqui, explicitamente. O que é dinâmico é *quais estão ativos por org* — isso é dado
//! no Postgres (`admin_feature_flag`), não código.
//!
//! Route mounting still lives in `api_router()`; migrar cada `.merge(...)` pra consumir este
//! catálogo é o strangler-fig incremental (contínuo), não parte do R0.

use dsoc_admin::permissions::{keys, ADMINISTRATOR};
use dsoc_app::manifest::{
    ModuleManifest, NavItem, NavSlot, PermKind, PermissionCategory, PermissionDef,
};
use dsoc_core::VerificationLevel;

/// Shorthand for a managed (role-gated) permission at Directory level.
const fn managed(
    key: &'static str,
    label: &'static str,
    category: PermissionCategory,
) -> PermissionDef {
    PermissionDef {
        key,
        label,
        category,
        min_level: VerificationLevel::Directory,
        kind: PermKind::Managed,
    }
}

const fn nav(label: &'static str, href: &'static str, order: i16) -> NavItem {
    NavItem {
        label,
        href,
        slot: NavSlot::Primary,
        order,
    }
}

/// The full registry, newest concerns last. Order is display order in the admin module list.
pub static CATALOG: &[ModuleManifest] = &[
    // --- Core (sempre on; gateable=false) --------------------------------------------------
    ModuleManifest {
        id: "auth",
        title: "Identidade & convites",
        core: true,
        flag_key: "module.auth",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[managed(
            keys::INVITES_MANAGE,
            "Gerenciar convites",
            PermissionCategory::Invites,
        )],
        nav: &[],
    },
    ModuleManifest {
        id: "admin",
        title: "Administração da plataforma",
        core: true,
        flag_key: "module.admin",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[
            PermissionDef {
                key: ADMINISTRATOR,
                label: "Administrador (acesso total)",
                category: PermissionCategory::Special,
                min_level: VerificationLevel::Directory,
                kind: PermKind::Managed,
            },
            managed(
                keys::VIEW_DASHBOARD,
                "Ver painel",
                PermissionCategory::Administration,
            ),
            managed(
                keys::VIEW_AUDIT_LOG,
                "Ver log de auditoria",
                PermissionCategory::Moderation,
            ),
            managed(
                keys::ROLES_MANAGE,
                "Gerenciar papéis",
                PermissionCategory::Administration,
            ),
            managed(
                keys::ORGS_MANAGE,
                "Gerenciar organizações",
                PermissionCategory::Administration,
            ),
            managed(
                keys::FLAGS_MANAGE,
                "Gerenciar módulos/flags",
                PermissionCategory::Administration,
            ),
            managed(
                keys::USERS_VIEW,
                "Ver contas",
                PermissionCategory::Moderation,
            ),
            managed(
                keys::USERS_MANAGE,
                "Gerenciar contas",
                PermissionCategory::Moderation,
            ),
            managed(
                keys::USERS_ACCESS,
                "Acesso a dados de conta",
                PermissionCategory::Moderation,
            ),
            managed(
                keys::ANNOUNCEMENTS_MANAGE,
                "Gerenciar anúncios",
                PermissionCategory::Administration,
            ),
            managed(
                keys::EMAIL_TEMPLATES_MANAGE,
                "Gerenciar modelos de e-mail",
                PermissionCategory::Administration,
            ),
            managed(
                keys::WEBHOOKS_MANAGE,
                "Gerenciar webhooks",
                PermissionCategory::Administration,
            ),
        ],
        nav: &[NavItem {
            label: "Admin",
            href: "/admin",
            slot: NavSlot::AdminMenu,
            order: 0,
        }],
    },
    ModuleManifest {
        id: "moderation",
        title: "Moderação & denúncias",
        core: true,
        flag_key: "module.moderation",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[
            managed(
                keys::REPORTS_MANAGE,
                "Gerenciar denúncias",
                PermissionCategory::Moderation,
            ),
            managed(
                keys::CONTENT_MODERATE,
                "Remover conteúdo (moderação)",
                PermissionCategory::Moderation,
            ),
        ],
        nav: &[],
    },
    ModuleManifest {
        id: "notify",
        title: "Notificações & e-mail",
        core: true,
        flag_key: "module.notify",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "events",
        title: "Barramento de eventos",
        core: true,
        flag_key: "module.events",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "storage",
        title: "Mídia",
        core: true,
        flag_key: "module.storage",
        default_enabled: true,
        gateable: false,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    // --- Módulos plugáveis (gateable=true) -------------------------------------------------
    ModuleManifest {
        id: "federation",
        title: "Federação (ActivityPub)",
        core: false,
        flag_key: "module.federation",
        default_enabled: true,
        gateable: true,
        depends_on: &[],
        permissions: &[managed(
            keys::FEDERATION_MANAGE,
            "Gerenciar federação",
            PermissionCategory::Administration,
        )],
        nav: &[nav("Explorar", "/explorar", 30)],
    },
    ModuleManifest {
        id: "forums",
        title: "Fóruns",
        core: false,
        flag_key: "module.forums",
        default_enabled: true,
        gateable: true,
        depends_on: &[],
        permissions: &[managed(
            keys::FORUMS_MODERATE,
            "Moderar fóruns",
            PermissionCategory::Moderation,
        )],
        nav: &[nav("Fóruns", "/f/", 40)],
    },
    ModuleManifest {
        id: "proposals",
        title: "Propostas",
        core: false,
        flag_key: "module.proposals",
        default_enabled: true,
        gateable: true,
        depends_on: &["mandates"],
        permissions: &[],
        nav: &[nav("Propostas", "/propostas", 20)],
    },
    ModuleManifest {
        id: "votes",
        title: "Votação",
        core: false,
        flag_key: "module.votes",
        default_enabled: true,
        gateable: false, // cluster do loop cívico — locked no R0 (emenda P2.3)
        depends_on: &["proposals"],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "comments",
        title: "Comentários",
        core: false,
        flag_key: "module.comments",
        default_enabled: true,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "consultations",
        title: "Consultas",
        core: false,
        flag_key: "module.consultations",
        default_enabled: true,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[nav("Consultas", "/consultas", 50)],
    },
    ModuleManifest {
        id: "mandates",
        title: "Mandatos & gabinetes",
        core: false,
        flag_key: "module.mandates",
        default_enabled: true,
        gateable: false, // registro mandate é core no baseline (emenda P2.1)
        depends_on: &[],
        permissions: &[],
        nav: &[nav("Políticos", "/politicos", 10)],
    },
    ModuleManifest {
        id: "consequence",
        title: "Consequência (SLA)",
        core: false,
        flag_key: "module.consequence",
        default_enabled: true,
        gateable: false, // parte do loop cívico — locked
        depends_on: &["proposals", "mandates"],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "scorecard",
        title: "Placar de promessas",
        core: false,
        flag_key: "module.scorecard",
        default_enabled: true,
        gateable: true,
        depends_on: &["mandates"],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "accountability",
        title: "Prestação de contas",
        core: false,
        flag_key: "module.accountability",
        default_enabled: true,
        gateable: true,
        depends_on: &["mandates"],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "assemblies",
        title: "Assembleias",
        core: false,
        flag_key: "module.assemblies",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "initiatives",
        title: "Iniciativas populares",
        core: false,
        flag_key: "module.initiatives",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "processes",
        title: "Processos participativos",
        core: false,
        flag_key: "module.processes",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "budgets",
        title: "Orçamento participativo",
        core: false,
        flag_key: "module.budgets",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "surveys",
        title: "Enquetes",
        core: false,
        flag_key: "module.surveys",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
    ModuleManifest {
        id: "meetings",
        title: "Reuniões",
        core: false,
        flag_key: "module.meetings",
        default_enabled: false,
        gateable: true,
        depends_on: &[],
        permissions: &[],
        nav: &[],
    },
];

/// Find a manifest by id.
#[must_use]
pub fn find(id: &str) -> Option<&'static ModuleManifest> {
    CATALOG.iter().find(|m| m.id == id)
}

/// Every permission declared across the catalog, deduped by key, for the R4 matrix.
#[must_use]
pub fn permission_catalog() -> Vec<PermissionDef> {
    let mut out: Vec<PermissionDef> = Vec::new();
    for m in CATALOG {
        for p in m.permissions {
            if !out.iter().any(|e| e.key == p.key) {
                out.push(*p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ids_and_flag_keys_are_unique() {
        let mut ids = BTreeSet::new();
        let mut flags = BTreeSet::new();
        for m in CATALOG {
            assert!(ids.insert(m.id), "id duplicado: {}", m.id);
            assert!(flags.insert(m.flag_key), "flag duplicada: {}", m.flag_key);
            assert_eq!(m.flag_key, format!("module.{}", m.id), "convenção flag_key");
        }
    }

    #[test]
    fn depends_on_resolves_and_is_acyclic() {
        let ids: BTreeSet<&str> = CATALOG.iter().map(|m| m.id).collect();
        for m in CATALOG {
            for dep in m.depends_on {
                assert!(ids.contains(dep), "{} depende de {} inexistente", m.id, dep);
                assert_ne!(*dep, m.id, "{} depende de si mesmo", m.id);
            }
        }
        // Grafo raso: nenhuma dep aponta de volta (checagem de ciclo simples).
        for m in CATALOG {
            for dep in m.depends_on {
                let dep_mod = CATALOG.iter().find(|x| x.id == *dep).unwrap();
                assert!(
                    !dep_mod.depends_on.contains(&m.id),
                    "ciclo entre {} e {}",
                    m.id,
                    dep
                );
            }
        }
    }

    #[test]
    fn every_known_permission_key_is_declared() {
        let declared: BTreeSet<&str> = CATALOG
            .iter()
            .flat_map(|m| m.permissions)
            .map(|p| p.key)
            .collect();
        for key in [
            ADMINISTRATOR,
            keys::VIEW_DASHBOARD,
            keys::VIEW_AUDIT_LOG,
            keys::ROLES_MANAGE,
            keys::ORGS_MANAGE,
            keys::FLAGS_MANAGE,
            keys::USERS_VIEW,
            keys::USERS_MANAGE,
            keys::USERS_ACCESS,
            keys::REPORTS_MANAGE,
            keys::CONTENT_MODERATE,
            keys::FORUMS_MODERATE,
            keys::FEDERATION_MANAGE,
            keys::ANNOUNCEMENTS_MANAGE,
            keys::EMAIL_TEMPLATES_MANAGE,
            keys::WEBHOOKS_MANAGE,
            keys::INVITES_MANAGE,
        ] {
            assert!(declared.contains(key), "chave sem manifesto dono: {key}");
        }
    }

    #[test]
    fn core_modules_are_not_gateable() {
        for m in CATALOG {
            if m.core {
                assert!(!m.gateable, "módulo core {} não pode ser gateable", m.id);
            }
        }
    }
}
