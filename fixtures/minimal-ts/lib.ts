/** Tiny fixture so `ast-scan` can run on this repo (Rust sources are not scanned). */
export function greet(name: string): string {
  if (name.length === 0) {
    return "hello";
  }
  return `hello, ${name}`;
}
