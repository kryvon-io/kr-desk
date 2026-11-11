//! Serviço Windows do KR Desk.
//!
//! Um processo comum não controla janelas de administrador (UIPI), não vê a
//! área de trabalho segura (UAC, tela de bloqueio) e não pode mandar
//! Ctrl+Alt+Del. Para isso:
//!
//! 1. o **serviço** (`kr-desk.exe --service`, LocalSystem) acompanha a sessão
//!    do console e mantém nela um **agente**;
//! 2. o **agente** (`kr-desk.exe --agent`) roda como SYSTEM na sessão do
//!    usuário, segue a área de trabalho ativa (normal ↔ segura) e atende o
//!    app por um named pipe: entrega a tela capturada e aplica a entrada
//!    (ver [`kr_core::agent`]);
//! 3. Ctrl+Alt+Del: o agente pede ao serviço (pela saída padrão), que chama
//!    `SendSAS`.
//!
//! O app usa o agente quando ele existe ([`connect_agent`]); sem o serviço,
//! continua capturando e injetando por conta própria.
//!
//! Segurança: o pipe só aceita usuários interativos e só conversa com o
//! próprio `kr-desk.exe` instalado (o caminho do executável dos dois lados
//! precisa ser o mesmo, em Arquivos de Programas). O agente encerra quando o
//! serviço some (a entrada padrão fecha).

#[cfg(windows)]
mod agent;
#[cfg(windows)]
mod desktop;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod service;
#[cfg(windows)]
mod util;

#[cfg(windows)]
pub use agent::run_agent;
#[cfg(windows)]
pub use install::{install_service, stop_service, uninstall_service};
#[cfg(windows)]
pub use pipe::{PipeStream, connect_agent};
#[cfg(windows)]
pub use service::run_service;

/// Nome do serviço no Windows (`sc query KRDeskService`).
pub const SERVICE_NAME: &str = "KRDeskService";
