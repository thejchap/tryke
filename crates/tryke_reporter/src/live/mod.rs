#[cfg(not(feature = "terminal"))]
mod fallback;
#[cfg(feature = "terminal")]
mod terminal;

#[cfg(not(feature = "terminal"))]
pub use fallback::*;
#[cfg(feature = "terminal")]
pub use terminal::*;
