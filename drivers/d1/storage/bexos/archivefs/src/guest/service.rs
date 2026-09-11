use super::migration;

pub(crate) async fn serve(state: migration::Runtime) -> ! {
    super::serve_inner(state).await
}
