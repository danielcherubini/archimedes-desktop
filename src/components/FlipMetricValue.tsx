/**
 * A number that plays a one-shot CSS flip when its value changes (the
 * `key` remount re-triggers the animation). `motion-safe`-equivalent
 * guard lives in the CSS (see `index.css`).
 */
export default function FlipMetricValue({ value }: { value: number }) {
  return (
    <span key={value} className="flip-in inline-block">
      {value}
    </span>
  );
}
