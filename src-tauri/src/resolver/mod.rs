//! Future stream resolver boundary. yt-dlp will resolve URLs only; media transfer
//! will use Reqwest/Tokio. No subprocess or network request is made here.

use std::{future::Future, pin::Pin};

use url::Url;

pub struct ResolvedStream {
    pub media_url: Url,
    pub mime_type: String,
}

pub type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ResolvedStream, String>> + Send + 'a>>;

pub trait StreamResolver: Send + Sync {
    fn resolve<'a>(&'a self, youtube_url: &'a Url) -> ResolveFuture<'a>;
}
