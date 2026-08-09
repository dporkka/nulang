/**
 * Nulang Grammar for Lezer/CodeMirror 6
 * This is a simplified grammar for syntax highlighting
 */

import { ExternalTokenizer, ExternalTokenizerWrapper } from "@lezer/lr";
import { Delimited } from "@lezer/common";

// Token types
const nulangTokens = {
  comment: 0,
  string: 1,
  keyword: 2,
  builtin: 3,
  number: 4,
  operator: 5,
  variable: 6,
  function: 7,
  type: 8,
  effect: 9,
  capability: 10
};

// Keywords
const keywords = new Set([
  "if", "then", "else", "match", "with", "case", "handle", "resume",
  "receive", "after", "in", "perform", "spawn", "send", "ask",
  "actor", "behavior", "state", "type", "effect", "agent", "workflow",
  "import", "module", "pub", "let", "rec", "fn", "self", "unit",
  "nil", "true", "false", "and", "or", "not", "return", "break",
  "continue", "for", "while", "loop", "defer", "errdefer", "recover",
  "consume", "pipe", "as", "match", "with", "case", "default",
  "when", "unless", "try", "catch", "finally", "throw", "raise"
]);

// Built-in types
const builtins = new Set([
  "Int", "Float", "Bool", "String", "Unit", "Nil", "Actor",
  "Option", "Result", "Some", "None", "Ok", "Error",
  "Array", "Record", "Tuple", "Channel", "Signal", "Timer",
  "Future", "Promise", "Stream", "Iterator", "Map", "Set",
  "IO", "Debug", "Timer", "Signal", "Inference", "LLM", "Otp",
  "CRDT", "Supervisor", "Workflow", "Database", "Provider",
  "Fs", "Net", "Http", "Json", "Yaml", "Toml", "Regex",
  "Crypto", "Random", "Time", "Date", "Uuid", "Env", "Process",
  "Thread", "Mutex", "Semaphore", "Select"
]);

// Capabilities
const capabilities = new Set([
  "iso", "trn", "ref", "val", "box", "tag", "lineariso",
  "linear", "sendable", "movable", "copyable", "droppable"
]);

// Effects
const effects = new Set([
  "IO", "Timer", "Signal", "Actor", "LLM", "Provider",
  "Workflow", "Database", "Crdt", "Supervisor", "Otp",
  "Fs", "Net", "Http", "Json", "Yaml", "Toml", "Regex",
  "Crypto", "Random", "Time", "Date", "Uuid", "Env",
  "Process", "Thread", "Mutex", "Semaphore", "Channel",
  "Select"
]);

// External tokenizer for Nulang
export const nulangTokenizer = new ExternalTokenizer((input, stack) => {
  let { next, peek } = input;
  
  // Skip whitespace
  while (next < input.end && /\s/.test(String.fromCharCode(peek()))) {
    next++;
  }
  
  if (next >= input.end) return;
  
  // Comments
  if (peek() === 47 && input.peek(1) === 47) { // //
    next += 2;
    while (next < input.end && peek() !== 10) next++; // \n
    input.acceptToken(nulangTokens.comment);
    return;
  }
  
  // Strings
  if (peek() === 34) { // "
    next++;
    while (next < input.end) {
      if (peek() === 34) { next++; break; }
      if (peek() === 92) { next += 2; continue; } // escape
      next++;
    }
    input.acceptToken(nulangTokens.string);
    return;
  }
  
  // Numbers
  if (peek() >= 48 && peek() <= 57) { // 0-9
    let isFloat = false;
    while (next < input.end && peek() >= 48 && peek() <= 57) next++;
    if (peek() === 46) { // .
      next++;
      isFloat = true;
      while (next < input.end && peek() >= 48 && peek() <= 57) next++;
    }
    if (peek() === 101 || peek() === 69) { // e/E
      next++;
      if (peek() === 43 || peek() === 45) next++; // +/-
      while (next < input.end && peek() >= 48 && peek() <= 57) next++;
      isFloat = true;
    }
    input.acceptToken(nulangTokens.number);
    return;
  }
  
  // Identifiers and keywords
  if ((peek() >= 97 && peek() <= 122) || (peek() >= 65 && peek() <= 90) || peek() === 95) { // a-z, A-Z, _
    let start = next;
    next++;
    while (next < input.end) {
      let c = peek();
      if ((c >= 97 && c <= 122) || (c >= 65 && c <= 90) || (c >= 48 && c <= 57) || c === 95) {
        next++;
      } else {
        break;
      }
    }
    
    const ident = String.fromCharCode(...input.input.slice(start, next));
    
    if (keywords.has(ident)) {
      input.acceptToken(nulangTokens.keyword);
    } else if (builtins.has(ident)) {
      input.acceptToken(nulangTokens.builtin);
    } else if (capabilities.has(ident)) {
      input.acceptToken(nulangTokens.capability);
    } else if (effects.has(ident)) {
      input.acceptToken(nulangTokens.effect);
    } else if (ident[0] === ident[0].toUpperCase()) {
      input.acceptToken(nulangTokens.type);
    } else if (/^[a-z][a-zA-Z0-9_]*$/.test(ident)) {
      // Check if it's followed by ( for function call
      let savedNext = next;
      while (savedNext < input.end && /\s/.test(String.fromCharCode(input.input[savedNext]))) savedNext++;
      if (input.input[savedNext] === 40) { // (
        input.acceptToken(nulangTokens.function);
      } else {
        input.acceptToken(nulangTokens.variable);
      }
    } else {
      input.acceptToken(nulangTokens.variable);
    }
    return;
  }
  
  // Operators
  if (/[+\-*/%<>=!|&^]/.test(String.fromCharCode(peek()))) {
    next++;
    // Multi-char operators
    if (peek() === 61) { next++; } // =
    if (peek() === 62) { next++; } // >
    if (peek() === 60) { next++; } // <
    if (peek() === 124) { next++; } // |
    if (peek() === 38) { next++; } // &
    input.acceptToken(nulangTokens.operator);
    return;
  }
  
  // Arrow ->
  if (peek() === 45 && input.peek(1) === 62) {
    next += 2;
    input.acceptToken(nulangTokens.operator);
    return;
  }
  
  // Arrow =>
  if (peek() === 61 && input.peek(1) === 62) {
    next += 2;
    input.acceptToken(nulangTokens.operator);
    return;
  }
  
  // Pipe |>
  if (peek() === 124 && input.peek(1) === 62) {
    next += 2;
    input.acceptToken(nulangTokens.operator);
    return;
  }
  
  // Punctuation
  next++;
}, {
  contextual: true,
  fallback: true
});

// Create parser using Lezer's parseMixed
export function createParser() {
  return parser.configure({
    props: [
      nulangLanguage.data.of({
        closeBrackets: { brackets: ["(", "[", "{", "'", '"'] },
        indentOnInput: /^\s*(?:case|default|else|catch|finally)\b/
      })
    ]
  });
}

// Language support
export const nulangSupport = new LanguageSupport(nulangLanguage, [
  syntaxHighlighting(nulangHighlightStyle)
]);