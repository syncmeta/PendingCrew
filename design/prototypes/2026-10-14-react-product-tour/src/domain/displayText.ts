/** Product terminology only; persisted records, identifiers and source documents stay intact. */
export function productText(text: string): string {
  return text
    .replace(/真人\s*Todo/gi, "人待办")
    .replace(/AI\s*Todo/gi, "AI待办")
    .replace(/Todo/gi, "待办");
}
