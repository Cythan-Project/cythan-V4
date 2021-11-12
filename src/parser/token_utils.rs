use std::collections::VecDeque;

use super::{expression::TokenProcessor, Token};

pub fn split_simple(tokens: VecDeque<Token>, token: &Token) -> Vec<VecDeque<Token>> {
    split_complex(tokens, |t| {
        if t == token {
            SplitAction::SplitConsume
        } else {
            SplitAction::None
        }
    })
}

pub enum SplitAction {
    None,
    Split,
    SplitConsume,
}

pub fn split_complex(
    tokens: VecDeque<Token>,
    split_rule: impl Fn(&Token) -> SplitAction,
) -> Vec<VecDeque<Token>> {
    let mut a = Vec::new();
    let mut b = VecDeque::new();
    for c in tokens {
        match split_rule(&c) {
            SplitAction::None => {
                b.push_back(c);
            }
            SplitAction::Split => {
                b.push_back(c);
                a.push(b);
                b = VecDeque::new();
            }
            SplitAction::SplitConsume => {
                if !b.is_empty() {
                    a.push(b);
                }
                b = VecDeque::new();
            }
        }
    }
    if !b.is_empty() {
        a.push(b);
    }
    a
}

pub fn take_until(tokens: &mut VecDeque<Token>, until: impl Fn(&Token) -> bool) -> VecDeque<Token> {
    let mut i = VecDeque::new();
    loop {
        if let Some(e) = tokens.get_token() {
            if !until(&e) {
                i.push_back(e);
            } else {
                tokens.push_front(e);
                break;
            }
        } else {
            break;
        }
    }
    i
}
