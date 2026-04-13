//! Generated icon identifiers for `assets/icons/*.{svg,png}`.
//!
//! This avoids sprinkling stringly-typed asset paths (e.g. `"icons/foo.svg"`) throughout the code.

macro_rules! asset_icons {
    (
        $vis:vis enum $name:ident {
            $(
                $variant:ident => $path:literal,
            )*
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $($variant,)*
        }

        impl $name {
            #[inline]
            pub const fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => $path,)*
                }
            }
        }

        impl From<$name> for gpui::SharedString {
            #[inline]
            fn from(icon: $name) -> Self {
                icon.path().into()
            }
        }

        impl From<$name> for gpui::ImageSource {
            #[inline]
            fn from(icon: $name) -> Self {
                icon.path().into()
            }
        }

        impl AsRef<str> for $name {
            #[inline]
            fn as_ref(&self) -> &str {
                self.path()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.path())
            }
        }
    };
}

include!(concat!(env!("OUT_DIR"), "/termua_icons.rs"));
