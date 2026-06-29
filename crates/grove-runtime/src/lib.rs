//! PHP runtime + FPM pool management (PRD §6.4).
//!
//! Grove can use any `php-fpm` binary: a downloaded static build, a system one,
//! or a user-registered binary with extra extensions (`grove php register`) —
//! directly addressing Herd's biggest limitation. Pools are started lazily and
//! reaped after inactivity to keep idle RAM low (PRD §7).

pub mod fpm;
pub mod install;
pub mod node;
pub mod registry;

pub use fpm::{FpmManager, FpmPool};
pub use install::{install as install_php, InstallError};
pub use node::{install as install_node, NodeBuild, NodeRegistry};
pub use registry::{PhpBuild, PhpRegistry};
