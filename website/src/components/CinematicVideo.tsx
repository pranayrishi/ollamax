"use client";

import { useEffect, useRef } from "react";

const VIDEO_SRC =
  "https://d8j0ntlcm91z4.cloudfront.net/user_38xzZboKViGWJOttwIXH07lWA1P/hf_20260314_131748_f2ca2a28-fed7-44c8-b9a9-bd9acdd5ec31.mp4";

/** Decorative, autoplaying hero media. Reduced-motion visitors receive the
 * deep-navy fallback instead of an animated scene. */
export function CinematicVideo() {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const video = videoRef.current;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    if (!video) return;

    const syncPlayback = () => {
      if (preference.matches) {
        video.pause();
      } else {
        void video.play().catch(() => {
          // Browsers may defer autoplay until visible. The solid background
          // remains a deliberate, readable fallback in that case.
        });
      }
    };

    syncPlayback();
    preference.addEventListener("change", syncPlayback);
    return () => preference.removeEventListener("change", syncPlayback);
  }, []);

  return (
    <video
      ref={videoRef}
      autoPlay
      loop
      muted
      playsInline
      preload="metadata"
      aria-hidden="true"
      tabIndex={-1}
      className="pointer-events-none absolute inset-0 z-0 h-full w-full object-cover"
    >
      <source src={VIDEO_SRC} type="video/mp4" />
    </video>
  );
}
