import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";

export default function StarError() {
  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-2xl px-4 py-20 text-center">
        <h1 className="text-2xl font-bold text-zinc-50">Something went wrong</h1>
        <p className="mt-2 text-zinc-400">
          The starring request couldn&rsquo;t be completed. Nothing was changed. Start again from the
          Hub in the app.
        </p>
      </main>
      <Footer />
    </>
  );
}
