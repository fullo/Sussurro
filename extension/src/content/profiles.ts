/* The name-observer profile of each meeting platform (#131, #245, #246). */
import type { Platform } from "../shared/platform";
import { MEET_PROFILE } from "./meet/profile";
import type { PlatformProfile } from "./names/profile";
import { TEAMS_PROFILE } from "./teams/profile";
import { ZOOM_PROFILE } from "./zoom/profile";

export const PROFILES: Readonly<Record<Platform, PlatformProfile>> = {
  meet: MEET_PROFILE,
  teams: TEAMS_PROFILE,
  zoom: ZOOM_PROFILE,
};

/** The profile for a page's platform (null: not a meeting page). */
export function profileFor(platform: Platform | null): PlatformProfile | null {
  return platform ? PROFILES[platform] : null;
}
