export function shout(text) {
  return text.toUpperCase() + "!";
}

export function neverImported() {
  return "tree shaking drops this";
}
