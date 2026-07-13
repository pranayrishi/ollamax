import { Nav } from "@/components/Nav";
import { Hero } from "@/components/Hero";
import { Features } from "@/components/Features";
import { HowItWorks } from "@/components/HowItWorks";
import { Comparison } from "@/components/Comparison";
import { Privacy } from "@/components/Privacy";
import { FAQ } from "@/components/FAQ";
import { CTA } from "@/components/CTA";
import { Footer } from "@/components/Footer";
import { CinematicVideo } from "@/components/CinematicVideo";

export default function Home() {
  return (
    <>
      <main id="main" className="site-page overflow-hidden">
        <section className="relative min-h-screen overflow-hidden bg-background">
          <CinematicVideo />
          <Nav />
          <Hero />
        </section>
        <Features />
        <HowItWorks />
        <Comparison />
        <Privacy />
        <FAQ />
        <CTA />
      </main>
      <Footer />
    </>
  );
}
