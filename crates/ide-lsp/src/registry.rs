//! Which language server to run for a file.
//!
//! The table is data, not code: adding a language is a row, and a server that is
//! not installed is reported to the user instead of being fatal — editing works
//! without it.

use std::path::Path;

/// One language server as it is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Shown in the status bar and in messages.
    pub name: &'static str,
    /// The program to run; looked up in `PATH`.
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// The `languageId` the server expects in `textDocument/didOpen`.
    pub language_id: &'static str,
    /// Extensions this server handles, without the dot.
    pub extensions: &'static [&'static str],
}

/// The servers this build knows about.
pub const SERVERS: &[ServerConfig] = &[
    ServerConfig {
        name: "rust-analyzer",
        command: "rust-analyzer",
        args: &[],
        language_id: "rust",
        extensions: &["rs"],
    },
    ServerConfig {
        name: "pyright",
        command: "pyright-langserver",
        args: &["--stdio"],
        language_id: "python",
        extensions: &["py"],
    },
    ServerConfig {
        name: "typescript-language-server",
        command: "typescript-language-server",
        args: &["--stdio"],
        language_id: "typescript",
        extensions: &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
    },
    ServerConfig {
        name: "gopls",
        command: "gopls",
        args: &[],
        language_id: "go",
        extensions: &["go"],
    },
    ServerConfig {
        name: "clangd",
        command: "clangd",
        args: &[],
        language_id: "c",
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp"],
    },
];

/// The server for a path, if the extension is one we know.
pub fn server_for(path: &Path) -> Option<&'static ServerConfig> {
    let extension = path.extension()?.to_str()?;
    SERVERS.iter().find(|server| {
        server
            .extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

/// Whether the program of a server can be found in `PATH`.
pub fn is_installed(server: &ServerConfig) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        let candidate = directory.join(server.command);
        candidate.is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_files_get_rust_analyzer() {
        let server = server_for(Path::new("crates/andy-ide/src/main.rs")).expect("a server");
        assert_eq!(server.name, "rust-analyzer");
        assert_eq!(server.language_id, "rust");
    }

    #[test]
    fn extensions_match_without_case() {
        let server = server_for(Path::new("a/MAIN.RS")).expect("a server");
        assert_eq!(server.name, "rust-analyzer");
    }

    #[test]
    fn the_more_specific_language_wins_over_the_extension_list() {
        let server = server_for(Path::new("app/x.tsx")).expect("a server");
        assert_eq!(server.name, "typescript-language-server");
        let server = server_for(Path::new("main.c")).expect("a server");
        assert_eq!(server.name, "clangd");
    }

    #[test]
    fn a_file_with_an_unknown_extension_has_no_server() {
        assert!(server_for(Path::new("README.md")).is_none());
        assert!(server_for(Path::new("Makefile")).is_none());
    }

    #[test]
    fn every_server_has_a_command_and_a_language() {
        for server in SERVERS {
            assert!(!server.command.is_empty(), "{}", server.name);
            assert!(!server.language_id.is_empty(), "{}", server.name);
            assert!(!server.extensions.is_empty(), "{}", server.name);
        }
    }
}
