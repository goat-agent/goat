import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

// react-markdown escapes raw HTML, so model text cannot inject markup.
const components: Components = {
  a: ({ node: _node, ...props }) => (
    <a
      {...props}
      target="_blank"
      rel="noopener noreferrer"
      className="underline underline-offset-2"
    />
  ),
  ul: ({ node: _node, ...props }) => <ul {...props} className="list-disc ps-5" />,
  ol: ({ node: _node, ...props }) => <ol {...props} className="list-decimal ps-5" />,
  pre: ({ node: _node, ...props }) => (
    <pre {...props} className="overflow-x-auto rounded-md bg-background/60 p-3 text-[13px]" />
  ),
  code: ({ node: _node, ...props }) => (
    <code {...props} className="rounded bg-background/60 px-1 py-0.5 font-mono text-[13px]" />
  ),
};

export function Markdown({ text }: { text: string }) {
  return (
    <div className="flex flex-col gap-2 [overflow-wrap:anywhere]">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
