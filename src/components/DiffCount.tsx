import FlipMetricValue from "./FlipMetricValue";

/**
 * A `+N -M` change stat: green additions, red removals, `font-mono
 * tabular-nums`, each number flip-animating on change.
 */
export default function DiffCount({
  stat,
}: {
  stat: { added: number; removed: number };
}) {
  return (
    <span className="inline-flex items-center gap-1 font-mono text-ui-sm leading-none tabular-nums">
      {stat.added > 0 && (
        <span className="text-success">
          +<FlipMetricValue value={stat.added} />
        </span>
      )}
      {stat.removed > 0 && (
        <span className="text-destructive">
          -<FlipMetricValue value={stat.removed} />
        </span>
      )}
    </span>
  );
}
