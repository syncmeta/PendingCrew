import { MessageText } from "./MessageText";
/** Small safe text renderer for authored product documents. No raw HTML evaluation. */
export function DocumentContent({ body }: { body: string }) {
  return (
    <div className="document-content">
      {body.split("\n\n").map((block, index) => {
        if (block.startsWith("## "))
          return <h4 key={index}>{block.slice(3)}</h4>;
        if (block.startsWith("- "))
          return (
            <ul key={index}>
              {block.split("\n").map((line, i) => (
                <li key={i}>
                  <MessageText text={line.replace(/^- /, "")} />
                </li>
              ))}
            </ul>
          );
        return (
          <p key={index}>
            <MessageText text={block} />
          </p>
        );
      })}
    </div>
  );
}
