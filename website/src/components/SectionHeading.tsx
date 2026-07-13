export function SectionHeading({
  eyebrow,
  title,
  subtitle,
  center = true,
}: {
  eyebrow?: string;
  title: string;
  subtitle?: string;
  center?: boolean;
}) {
  return (
    <div className={center ? "mx-auto max-w-2xl text-center" : "max-w-2xl"}>
      {eyebrow && (
        <p className="eyebrow mb-4">
          {eyebrow}
        </p>
      )}
      <h2 className="font-display text-4xl leading-[0.98] tracking-[-0.04em] text-foreground sm:text-5xl">{title}</h2>
      {subtitle && <p className="mt-5 text-base leading-relaxed text-muted-foreground">{subtitle}</p>}
    </div>
  );
}
