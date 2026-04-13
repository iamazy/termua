mod assets;
mod format;
mod icons;
mod limiter;
mod toast;
mod transfer;

pub use assets::TermuaAssets;
pub use format::format_bytes;
pub use icons::TermuaIcon;
pub use limiter::{
    PermitPool, PermitPoolPermit, set_sftp_upload_permit_pool_max,
    set_sftp_upload_permit_pool_max_in_app, sftp_upload_permit_pool,
    sftp_upload_permit_pool_in_app,
};
pub use toast::{Toast, ToastRenderOptions, ToastVariant, render_toast};
pub use transfer::{
    TransferFooterBar, render_transfer_footer_bar, render_transfer_footer_bar_with_action,
};

pub type CloseFn<T> = std::rc::Rc<dyn Fn(&mut T, &mut gpui::Context<T>)>;
