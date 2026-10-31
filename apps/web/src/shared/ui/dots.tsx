/** Three dots that pulse in turn, for "working" and "writing a reply". */
export function Dots() {
  return (
    <span className="inline-flex items-center gap-[3px]">
      {[0, 200, 400].map((delay) => (
        <span
          key={delay}
          className="size-[5px] animate-pulse rounded-full bg-current motion-reduce:animate-none"
          style={{ animationDelay: `${delay}ms` }}
        />
      ))}
    </span>
  );
}
