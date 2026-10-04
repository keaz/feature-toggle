use super::App;
use crate::context::Context;
use crate::error::CliError;
use crate::output::{Kind, Outcome};

pub async fn run(app: &mut App<'_>) -> Result<Outcome, CliError> {
    let files = app.files()?;
    let context = Context::anonymous(app.settings(&files)?, app.paths.clone())?;
    Ok(Outcome::new(context.api.get(&["health"], &[]).await?, Kind::Object))
}
