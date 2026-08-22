//! Manual diagnostic probe: prints the real rebuild error chain.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DEBUG_DB_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;
    let config = melodee_search::SearchConfig::from_env()?;
    println!("config search_root: {:?}", config.search_root);
    match melodee_search::request_rebuild(&config, &pool, "probe").await {
        Ok(job) => println!("requested job {:?}", job.job_id),
        Err(error) => {
            println!("request_rebuild error: {error:?}");
            return Ok(());
        }
    }
    match melodee_search::run_rebuild_once(&config, &pool).await {
        Ok(result) => println!("rebuild ok: {result:?}"),
        Err(error) => println!("rebuild error: {error:?}"),
    }
    Ok(())
}
