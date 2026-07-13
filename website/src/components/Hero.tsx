import { Button } from "@/components/ui/button";

export function Hero() {
  return (
    <section className="relative z-10 flex min-h-[calc(100svh-112px)] items-center justify-center px-6 pb-40 pt-32 text-center sm:min-h-[calc(100svh-128px)]">
      <div className="mx-auto flex max-w-7xl flex-col items-center">
        <h1
          className="animate-fade-rise max-w-7xl text-5xl leading-[0.95] tracking-[-2.46px] text-foreground sm:text-7xl md:text-8xl"
          style={{ fontFamily: "var(--font-display)" }}
        >
          Where <em className="not-italic text-muted-foreground">dreams</em> rise{" "}
          <em className="not-italic text-muted-foreground">through the silence.</em>
        </h1>

        <p className="animate-fade-rise-delay mt-8 max-w-2xl text-base leading-relaxed text-muted-foreground sm:text-lg">
          We&apos;re designing tools for deep thinkers, bold creators, and quiet rebels. Amid the
          chaos, we build digital spaces for sharp focus and inspired work.
        </p>

        <Button asChild variant="glass" size="lg" className="animate-fade-rise-delay-2 mt-12 px-14 py-5">
          <a href="#download">Begin Journey</a>
        </Button>
      </div>
    </section>
  );
}
