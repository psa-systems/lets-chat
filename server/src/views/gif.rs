//! LC-488: GIF picker result grid fragment.

use crate::gif::GifResult;
use crate::i18n::filters; // in-scope for the |t template filter.
use askama::Template;

#[derive(Template)]
#[template(path = "gif_results.html")]
pub struct GifResultsFragment<'a> {
    pub results: &'a [GifResult],
}
