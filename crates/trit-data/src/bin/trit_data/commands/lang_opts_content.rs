use trit_data::content::ContentFilter;

#[path = "bits_opts.rs"]
mod bits_opts;
#[path = "content_base_opts.rs"]
mod content_base_opts;
#[path = "format_opts.rs"]
mod format_opts;
#[path = "label_opts.rs"]
mod label_opts;
#[path = "quality_opts.rs"]
mod quality_opts;

pub fn apply(filter: &mut ContentFilter, flag: &str, argv: &[String], i: &mut usize) -> bool {
    content_base_opts::apply(filter, flag, argv, i)
        || label_opts::apply(filter, flag, argv, i)
        || quality_opts::apply(filter, flag, argv, i)
        || format_opts::apply(filter, flag, argv, i)
        || bits_opts::apply(filter, flag, argv, i)
}
