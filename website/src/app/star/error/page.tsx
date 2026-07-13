import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";

export default function StarError() {
  return (
    <>
      <Nav />
      <main id="main" className="page-frame max-w-2xl text-center">
        <h1 className="page-title text-4xl">Something went wrong</h1>
        <p className="page-lede mx-auto text-base">
          The starring request couldn&rsquo;t be completed. Nothing was changed. Start again from the
          Hub in the app.
        </p>
      </main>
      <Footer />
    </>
  );
}
