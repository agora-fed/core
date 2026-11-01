//! Standalone migration runner for systemd deploys.
//! Reads DATABASE_URL, applies all pending migrations, and exits.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (e.g. postgres://agora@[::1]/agora)")?;
    let db = dsoc_db::connect(&database_url, 4).await?;
    dsoc_db::run_migrations(&db).await?;
    println!("migrations applied successfully");
    Ok(())
}
