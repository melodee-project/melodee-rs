use melodee_search::{SearchConfig, request_rebuild, run_rebuild_once};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DEBUG_DB_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let config = SearchConfig::from_env()?;
    println!("config root: {:?}", config.search_root);
    let _job = match request_rebuild(&config, &pool, "debug").await {
        Ok(job) => {
            println!("requested: {:?}", job.job_id);
            job
        }
        Err(error) => {
            println!("request_rebuild error: {error:?}");
            return Ok(());
        }
    };
    match run_rebuild_once(&config, &pool).await {
        Ok(id) => println!("rebuild done: {id:?}"),
        Err(error) => println!("run_rebuild_once error: {error:?}"),
    }
    Ok(())
}
