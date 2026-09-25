//! Which language a porthole's text is: iTerm2's order (a type hint, the file name, the
//! markdown and JSON sniffs, then a guess), with the fix that a hint matching nothing still
//! lets the sniffs run.

use std::sync::OnceLock;
use syntect::parsing::SyntaxSet;

/// What a porthole shows its text as.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Markdown,
    Json,
    Plain,
    /// A syntect syntax, by name.
    Code(String),
}

impl Language {
    /// The name the language list shows.
    pub fn name(&self) -> String {
        match self {
            Language::Markdown => "Markdown".into(),
            Language::Json => "JSON".into(),
            Language::Plain => "Plain text".into(),
            Language::Code(n) => n.clone(),
        }
    }
}

/// syntect's built-in grammars, loaded once.
pub fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn language_of(syntax: &str) -> Language {
    match syntax {
        "Markdown" | "MultiMarkdown" => Language::Markdown,
        "JSON" => Language::Json,
        "Plain Text" => Language::Plain,
        other => Language::Code(other.to_string()),
    }
}

/// Every language: Markdown, JSON, and Plain text first, then the grammars by name.
pub fn languages() -> Vec<Language> {
    let mut code: Vec<Language> = syntaxes().syntaxes().iter().map(|s| language_of(&s.name)).filter(|l| matches!(l, Language::Code(_))).collect();
    code.sort_by_key(|l| l.name().to_lowercase());
    code.dedup();
    let mut all = vec![Language::Markdown, Language::Json, Language::Plain];
    all.extend(code);
    all
}

fn push_unique(out: &mut Vec<Language>, l: Language) {
    if !out.contains(&l) {
        out.push(l);
    }
}

/// The languages a file extension (no dot) belongs to.
fn for_extension(ext: &str) -> Vec<Language> {
    let mut out = Vec::new();
    for s in syntaxes().syntaxes() {
        if s.file_extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
            push_unique(&mut out, language_of(&s.name));
        }
    }
    out
}

/// Short names iTerm2's list uses that are neither a grammar's name nor an extension.
const ALIASES: &[(&str, &str)] = &[
    ("shell", "Bourne Again Shell (bash)"),
    ("plaintext", "Plain Text"),
    ("text", "Plain Text"),
    ("golang", "Go"),
    ("csharp", "C#"),
    ("objc", "Objective-C"),
    ("objectivec", "Objective-C"),
];

const MIMES: &[(&str, &str)] = &[
    ("text/markdown", "Markdown"),
    ("application/json", "JSON"),
    ("text/plain", "Plain Text"),
    ("text/x-python", "Python"),
    ("application/javascript", "JavaScript"),
    ("text/javascript", "JavaScript"),
    ("text/html", "HTML"),
    ("text/css", "CSS"),
    ("application/xml", "XML"),
    ("text/xml", "XML"),
    ("text/x-shellscript", "Bourne Again Shell (bash)"),
];

/// A File= or Block `type`: `.ext` (possibly matching nothing: an empty list), a MIME
/// type, a language's name, or a short name. None when it is none of these.
pub fn for_type_hint(hint: &str) -> Option<Vec<Language>> {
    let hint = hint.trim();
    if let Some(ext) = hint.strip_prefix('.') {
        return Some(for_extension(ext));
    }
    let lower = hint.to_lowercase();
    if let Some((_, name)) = MIMES.iter().find(|(m, _)| *m == lower) {
        return Some(vec![language_of(name)]);
    }
    if let Some(l) = [Language::Markdown, Language::Json, Language::Plain].into_iter().find(|l| l.name().to_lowercase() == lower) {
        return Some(vec![l]);
    }
    if let Some(s) = syntaxes().syntaxes().iter().find(|s| s.name.to_lowercase() == lower) {
        return Some(vec![language_of(&s.name)]);
    }
    if let Some((_, name)) = ALIASES.iter().find(|(a, _)| *a == lower) {
        return Some(vec![language_of(name)]);
    }
    let by_ext = for_extension(&lower);
    (!by_ext.is_empty()).then_some(by_ext)
}

/// The languages of every dotted ending of a file name (`a.tar.gz`: `tar.gz`, then `gz`).
pub fn for_path(path: &str) -> Vec<Language> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let mut out = Vec::new();
    for (i, _) in name.match_indices('.') {
        for l in for_extension(&name[i + 1..]) {
            push_unique(&mut out, l);
        }
    }
    out
}

/// What detection found: the language, and the list the language button offers first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detected {
    pub language: Option<Language>,
    pub candidates: Vec<Language>,
}

/// iTerm2's markdown sniff: the text starts `# ` and a character.
fn looks_like_markdown(text: &str) -> bool {
    text.strip_prefix("# ").and_then(|r| r.chars().next()).is_some_and(|c| c != '\n' && c != '\r')
}

pub fn detect(text: &str, hint: Option<&str>, filename: Option<&str>) -> Detected {
    let mut candidates = Vec::new();
    if let Some(c) = hint.and_then(for_type_hint).filter(|c| !c.is_empty()) {
        if c.len() == 1 {
            return Detected { language: Some(c[0].clone()), candidates: c };
        }
        candidates = c;
    }
    if candidates.is_empty() {
        if let Some(c) = filename.map(for_path).filter(|c| !c.is_empty()) {
            if c.len() == 1 {
                return Detected { language: Some(c[0].clone()), candidates: c };
            }
            candidates = c;
        }
    }
    if candidates.is_empty() {
        if looks_like_markdown(text) {
            return Detected { language: Some(Language::Markdown), candidates };
        }
        if serde_json::from_str::<serde_json::Value>(text).is_ok() {
            return Detected { language: Some(Language::Json), candidates };
        }
    }
    let language = guess(text, &candidates);
    Detected { language: Some(language), candidates }
}

/// Words that mark a language, for text with no name or type: the language with the most
/// of them (2 at least) wins.
const KEYWORDS: &[(&str, &[&str])] = &[
    ("Python", &["def ", "import ", "self", "elif ", "None", "print(", "__init__", "    return"]),
    ("Rust", &["fn ", "let mut ", "impl ", "pub fn", "::", "println!", "-> ", "&mut "]),
    ("JavaScript", &["function ", "const ", "=> ", "console.log", "var ", "require(", "===", "document."]),
    ("C", &["#include", "int main", "printf(", "->", "struct ", "void ", "malloc("]),
    ("C++", &["std::", "#include <iostream>", "template<", "cout", "namespace ", "nullptr"]),
    ("C#", &["using System", "namespace ", "public class", "Console.WriteLine", "{ get;", "static void Main"]),
    ("Java", &["public static void main", "System.out.println", "import java", "extends ", "public class", "@Override"]),
    ("Go", &["package main", "func ", ":= ", "fmt.", "import (", "go func"]),
    ("Ruby", &["def ", "puts ", "require '", "do |", "attr_accessor", "\nend"]),
    ("Bourne Again Shell (bash)", &["#!/bin", "echo ", "\nfi", " then", "esac", "$(", "export "]),
    ("PHP", &["<?php", "$this->", "echo ", "function ", "->"]),
    ("SQL", &["SELECT ", " FROM ", " WHERE ", "INSERT INTO", "CREATE TABLE", "JOIN "]),
    ("HTML", &["<html", "<div", "<body", "<head", "</p>", "<!DOCTYPE"]),
    ("CSS", &["color:", "margin:", "px;", "font-", "padding:", "display:"]),
    ("XML", &["<?xml", "xmlns", "</"]),
    ("Lua", &["local ", "function ", " then", "\nend", "~="]),
    ("Perl", &["my $", "use strict", "sub ", "=~"]),
    ("Haskell", &[" :: ", " -> ", "where", "import qualified", "data ", "module "]),
    ("Makefile", &[".PHONY", "$(CC)", ":=", "\t$("]),
    ("YAML", &["---\n", ":\n  ", "\n- "]),
];

/// A guess for text nobody named: a first-line match (a shebang, `<?xml`), else keywords;
/// Plain text when nothing fits. With candidates, only they are considered.
pub fn guess(text: &str, candidates: &[Language]) -> Language {
    let allowed = |l: &Language| candidates.is_empty() || candidates.contains(l);
    let first = text.lines().next().unwrap_or("");
    if let Some(s) = syntaxes().find_syntax_by_first_line(first) {
        let l = language_of(&s.name);
        if allowed(&l) {
            return l;
        }
    }
    let mut best: Option<(usize, Language)> = None;
    for (name, words) in KEYWORDS {
        if syntaxes().find_syntax_by_name(name).is_none() {
            continue;
        }
        let l = language_of(name);
        if !allowed(&l) {
            continue;
        }
        let score = words.iter().filter(|w| text.contains(*w)).count();
        if score >= 2 && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, l));
        }
    }
    best.map_or(Language::Plain, |(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(name: &str) -> Language {
        Language::Code(name.to_string())
    }

    #[test]
    fn type_hints_follow_iterm2() {
        assert_eq!(for_type_hint(".py"), Some(vec![code("Python")]));
        assert_eq!(for_type_hint("text/markdown"), Some(vec![Language::Markdown]));
        assert_eq!(for_type_hint("application/json"), Some(vec![Language::Json]));
        assert_eq!(for_type_hint("text/plain"), Some(vec![Language::Plain]));
        assert_eq!(for_type_hint("Markdown"), Some(vec![Language::Markdown]));
        assert_eq!(for_type_hint("python"), Some(vec![code("Python")]));
        assert_eq!(for_type_hint("PYTHON"), Some(vec![code("Python")]), "names ignore case");
        assert_eq!(for_type_hint("bash"), Some(vec![code("Bourne Again Shell (bash)")]), "short names");
        assert_eq!(for_type_hint("nonsense"), None);
        assert_eq!(for_type_hint(".zzz"), Some(vec![]), "an unknown extension is an empty set");
    }

    #[test]
    fn filename_suffixes() {
        assert_eq!(for_path("README.md"), vec![Language::Markdown]);
        assert_eq!(for_path("src/main.rs"), vec![code("Rust")]);
        assert_eq!(for_path("data.json"), vec![Language::Json]);
        assert_eq!(for_path("notes.txt"), vec![Language::Plain]);
        assert!(for_path("Makefile.zzz").is_empty());
    }

    #[test]
    fn sniffs_markdown_then_json() {
        assert_eq!(detect("# Title\ntext", None, None).language, Some(Language::Markdown));
        assert_ne!(detect("#nospace", None, None).language, Some(Language::Markdown));
        assert_ne!(detect("# \nx", None, None).language, Some(Language::Markdown), "`# ` then a character");
        assert_eq!(detect("{\"a\": 1}", None, None).language, Some(Language::Json));
        assert_eq!(detect("42", None, None).language, Some(Language::Json), "fragments count");
        assert_eq!(detect("hello world", None, None).language, Some(Language::Plain));
    }

    #[test]
    fn empty_hint_still_sniffs() {
        assert_eq!(detect("# T", Some(".zzz"), None).language, Some(Language::Markdown));
        assert_eq!(detect("x = 1", Some("nonsense"), Some("a.py")).language, Some(code("Python")));
        assert_eq!(detect("# T", Some(".py"), None).language, Some(code("Python")), "a matching hint wins");
    }

    #[test]
    fn guess_uses_shebang_and_keywords() {
        assert_eq!(detect("#!/usr/bin/env python3\nx = 1\n", None, None).language, Some(code("Python")));
        assert_eq!(detect("fn main() {\n    let mut x = 1;\n    println!(\"{}\", x);\n}\n", None, None).language, Some(code("Rust")));
        assert_eq!(detect("SELECT a FROM t WHERE b = 1;", None, None).language, Some(code("SQL")));
        let d = detect("x", Some(".h"), None);
        assert!(d.candidates.len() > 1, "an extension with several languages gives candidates: {:?}", d.candidates);
    }
}
