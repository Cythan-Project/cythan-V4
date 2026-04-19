//! Cythan Language Server.
//!
//! Binary entry point + request/notification dispatcher. Each
//! LSP capability lives in its own module:
//!
//! | module        | concern                               |
//! |---------------|---------------------------------------|
//! | `state`       | Shared state kept between requests    |
//! | `text`        | Cursor / offset / identifier helpers  |
//! | `ast`         | AST walking + type inference          |
//! | `symbols`     | Cross-file symbol index               |
//! | `diagnostics` | `diagnose` → LSP push                 |
//! | `definition`  | `textDocument/definition`             |
//! | `hover`       | `textDocument/hover`                  |
//! | `completion`  | `textDocument/completion`             |
//! | `outline`     | Document + workspace symbols          |
//! | `references`  | `textDocument/references`             |

mod ast;
mod code_actions;
mod completion;
mod definition;
mod diagnostics;
mod hover;
mod outline;
mod references;
mod rename;
mod state;
mod symbols;
mod text;

use lsp_server::{Connection, Message};
use lsp_types::*;

use crate::state::State;

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    eprintln!("cythan-lsp: starting");
    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::FULL,
        )),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(
            CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            },
        )),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), ":".into()]),
            resolve_provider: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let init_params = connection
        .initialize(serde_json::to_value(server_capabilities)?)?;

    let mut state = State::from_init(&init_params);
    main_loop(&connection, &mut state)?;
    io_threads.join()?;
    Ok(())
}

fn main_loop(
    connection: &Connection,
    state: &mut State,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(connection, state, req);
            }
            Message::Notification(not) => handle_notification(connection, state, not),
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn handle_request(
    connection: &Connection,
    state: &State,
    req: lsp_server::Request,
) {
    use lsp_types::request::{
        CodeActionRequest, Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest,
        PrepareRenameRequest, References, Rename, Request as LspRequest,
        WorkspaceSymbolRequest,
    };
    let id = req.id.clone();
    let value = if req.method == GotoDefinition::METHOD {
        let loc = serde_json::from_value::<GotoDefinitionParams>(req.params)
            .ok()
            .and_then(|params| {
                definition::resolve_definition(
                    state,
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                )
            });
        match loc {
            Some(l) => serde_json::to_value(GotoDefinitionResponse::Scalar(l))
                .unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        }
    } else if req.method == Completion::METHOD {
        let items = serde_json::from_value::<CompletionParams>(req.params)
            .ok()
            .map(|params| {
                completion::resolve_completion(
                    state,
                    &params.text_document_position.text_document.uri,
                    params.text_document_position.position,
                )
            })
            .unwrap_or_default();
        serde_json::to_value(CompletionResponse::Array(items))
            .unwrap_or(serde_json::Value::Null)
    } else if req.method == HoverRequest::METHOD {
        let h = serde_json::from_value::<HoverParams>(req.params)
            .ok()
            .and_then(|params| {
                hover::resolve_hover(
                    state,
                    &params.text_document_position_params.text_document.uri,
                    params.text_document_position_params.position,
                )
            });
        serde_json::to_value(h).unwrap_or(serde_json::Value::Null)
    } else if req.method == DocumentSymbolRequest::METHOD {
        let syms = serde_json::from_value::<DocumentSymbolParams>(req.params)
            .ok()
            .map(|p| outline::document_symbols(state, &p.text_document.uri))
            .unwrap_or_default();
        serde_json::to_value(DocumentSymbolResponse::Nested(syms))
            .unwrap_or(serde_json::Value::Null)
    } else if req.method == WorkspaceSymbolRequest::METHOD {
        let syms = serde_json::from_value::<WorkspaceSymbolParams>(req.params)
            .ok()
            .map(|p| outline::workspace_symbols(state, &p.query))
            .unwrap_or_default();
        serde_json::to_value(syms).unwrap_or(serde_json::Value::Null)
    } else if req.method == References::METHOD {
        let refs = serde_json::from_value::<ReferenceParams>(req.params)
            .ok()
            .map(|p| {
                references::find_references(
                    state,
                    &p.text_document_position.text_document.uri,
                    p.text_document_position.position,
                    p.context.include_declaration,
                )
            })
            .unwrap_or_default();
        serde_json::to_value(refs).unwrap_or(serde_json::Value::Null)
    } else if req.method == CodeActionRequest::METHOD {
        let actions = serde_json::from_value::<CodeActionParams>(req.params)
            .ok()
            .map(|p| code_actions::resolve_code_actions(state, p))
            .unwrap_or_default();
        serde_json::to_value(actions).unwrap_or(serde_json::Value::Null)
    } else if req.method == PrepareRenameRequest::METHOD {
        let resp = serde_json::from_value::<TextDocumentPositionParams>(req.params)
            .ok()
            .and_then(|p| rename::prepare_rename(state, p));
        serde_json::to_value(resp).unwrap_or(serde_json::Value::Null)
    } else if req.method == Rename::METHOD {
        let resp = serde_json::from_value::<RenameParams>(req.params)
            .ok()
            .and_then(|p| rename::rename(state, p));
        serde_json::to_value(resp).unwrap_or(serde_json::Value::Null)
    } else {
        return;
    };
    let _ = connection.sender.send(Message::Response(lsp_server::Response {
        id,
        result: Some(value),
        error: None,
    }));
}

fn handle_notification(
    connection: &Connection,
    state: &mut State,
    not: lsp_server::Notification,
) {
    match not.method.as_str() {
        "textDocument/didOpen" => {
            if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(not.params)
            {
                state
                    .docs
                    .insert(p.text_document.uri.clone(), p.text_document.text.clone());
                diagnostics::publish_for(
                    connection,
                    state,
                    &p.text_document.uri,
                    &p.text_document.text,
                );
            }
        }
        "textDocument/didChange" => {
            if let Ok(mut p) =
                serde_json::from_value::<DidChangeTextDocumentParams>(not.params)
            {
                if let Some(change) = p.content_changes.pop() {
                    state
                        .docs
                        .insert(p.text_document.uri.clone(), change.text.clone());
                    diagnostics::publish_for(
                        connection,
                        state,
                        &p.text_document.uri,
                        &change.text,
                    );
                }
            }
        }
        "textDocument/didSave" => {
            if let Ok(p) = serde_json::from_value::<DidSaveTextDocumentParams>(not.params)
            {
                let uri = p.text_document.uri.clone();
                let text = p
                    .text
                    .clone()
                    .or_else(|| state.docs.get(&uri).cloned())
                    .unwrap_or_default();
                diagnostics::publish_for(connection, state, &uri, &text);
            }
        }
        "textDocument/didClose" => {
            if let Ok(p) =
                serde_json::from_value::<DidCloseTextDocumentParams>(not.params)
            {
                state.docs.remove(&p.text_document.uri);
                state.asts.remove(&p.text_document.uri);
                diagnostics::send_diagnostics(connection, &p.text_document.uri, vec![]);
            }
        }
        _ => {}
    }
}
