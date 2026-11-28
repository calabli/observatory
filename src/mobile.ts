import { useEffect, useState } from "react";

export const IS_ANDROID =
  typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);

export const IS_MOBILE_PREVIEW =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("mobile") === "1";

export function isMobilePlatform(): boolean {
  return IS_ANDROID || IS_MOBILE_PREVIEW;
}

/**
 * True on Android devices, `?mobile=1` preview URLs, or viewports <= `breakpoint` px.
 * Updates live when the window is resized or rotated.
 */
export function useIsMobile(breakpoint = 768): boolean {
  const [mobile, setMobile] = useState<boolean>(() => {
    if (isMobilePlatform()) return true;
    if (typeof window === "undefined") return false;
    return window.innerWidth <= breakpoint;
  });

  useEffect(() => {
    if (isMobilePlatform()) {
      setMobile(true);
      return;
    }
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia(`(max-width: ${breakpoint}px)`);
    const update = () => setMobile(mq.matches);
    update();
    mq.addEventListener("change", update);
    return () => mq.removeEventListener("change", update);
  }, [breakpoint]);

  return mobile;
}
