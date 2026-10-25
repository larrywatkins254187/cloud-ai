import { cx } from "./ui";

/** Unified diff with coloured lines. */
export function Diff({ text, className }: { text: string; className?: string }) {
  if (!text.trim()) return <div className="text-sm text-muted p-3">No changes</div>;
  return (
    <pre className={cx("text-[12px] leading-5 font-mono overflow-x-auto scroll-thin bg-code rounded-lg border border-border", className)}>
      {text.split("\n").map((l, i) => (
        <div
          key={i}
          className={cx(
            "px-3 whitespace-pre",
            l.startsWith("+") && !l.startsWith("+++") && "diff-add",
            l.startsWith("-") && !l.startsWith("---") && "diff-del",
            l.startsWith("@@") && "diff-hunk",
            (l.startsWith("diff ") || l.startsWith("index ")) && "text-muted",
          )}
        >
          {l || " "}
        </div>
      ))}
    </pre>
  );
}
