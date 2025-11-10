use clap::ValueEnum;

#[derive(ValueEnum, Clone, Copy)]
/// HTML template for markdown preview
pub enum Template {
    /// A classic, simple template
    Classic,

    /// A curriculum vitae template
    Cv,
}

impl AsRef<str> for Template {
    fn as_ref(&self) -> &str {
        match self {
            Template::Classic => "classic",
            Template::Cv => "cv",
        }
    }
}
