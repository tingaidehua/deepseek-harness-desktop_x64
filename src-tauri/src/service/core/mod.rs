//! Harness 内核管理。
//!
//! 内核来源：
//! - `local`：用户通过 CLI（npm/pnpm 全局安装）自行安装的 dsh，安装目录与
//!   配置（`$DSH_HOME`）都不归桌面端管理；
//! - `app`：桌面端预打包管理的 deepseek-harness-pkg 多版本副本。每个发行版
//!   位于 `dependencies/cores/<tag>` 不可变槽位；切换只提交活动 tag，不移动内核
//!   目录。旧版 `dependencies/dsh` 仅作为升级前回退。
//!
//! 启动优先级（需求）：本地内核存在时优先使用本地内核；未检测到才走预打包。
//! 用户在「内核」面板可显式切回预打包；显式选择持久化在 store 设置
//! （`active_core`），`None` = 自动（本地优先）。
//!
//! 本地内核更新通过其包管理器 CLI 完成（npm `update -g` / pnpm `add -g`），
//! 不触碰用户安装本身之外的文件。
//!
//! 模块划分（参考 `service/cli/`、`service/download/`）：
//! - [`local`]：本地内核发现（PATH/全局安装目录探测、包目录解析、更新本地内核）
//! - [`source`]：内核来源与活动入口（`CoreSource` / `HarnessCore` / 活动内核）
//! - [`version`]：预打包内核多版本管理（列出 / 切换 / 下载 / 卸载）

mod local;
mod source;
mod version;

pub use local::{local_core_package_dir, update_local_core};
// 以下重导出为对外公开 API（部分项当前链路未直接引用，属有意保留，见模块头）。
#[allow(unused_imports)]
pub use source::{
    active_core_dir, active_dsh_binary, active_source, active_version, CoreSource, HarnessCore,
};
pub use version::{download_version, list, remove_version, set_active};
