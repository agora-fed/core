//! # dsoc-forums — fóruns institucionais hierárquicos
//!
//! A tese da plataforma aplicada a fóruns (`/f/<caminho>`): a sociedade delibera,
//! as interações LOCAIS cruzam patamares configuráveis e cada patamar dispara um
//! e-mail à instituição responsável, com recibo público. Interações federadas
//! (fediverso) contam SEPARADO e nunca disparam envio.
//!
//! Estrutura espelha os demais components (proposals/debates): `domain` puro,
//! `queries` sqlx compile-time, `service` transacional, `http` axum.

pub mod domain;
pub mod http;
pub mod queries;
pub mod service;

pub use domain::{territorial_sections, NewTopic, TerritorialSection, MAX_DEPTH};
pub use http::routes;
pub use service::ForumService;
