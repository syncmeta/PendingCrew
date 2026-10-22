import { productText } from "../domain/displayText";
/** Limited inline emphasis, rendered as React text nodes rather than HTML. */
export function MessageText({ text }: { text: string }) {
  return (
    <>
      {productText(text)
        .split(/(\*\*[^*]+\*\*)/g)
        .map((part, index) =>
          part.startsWith("**") && part.endsWith("**") ? (
            <strong key={index}>{part.slice(2, -2)}</strong>
          ) : (
            part
          ),
        )}
    </>
  );
}
