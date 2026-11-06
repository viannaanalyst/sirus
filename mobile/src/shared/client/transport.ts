import type { DeviceProxy, DeviceHubAccess } from "./types";

export type Unlisten = () => void;

export interface Transport {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten>;
  /** Where an `<img>` loads a sent image's thumbnail by attachment id. Optional for test transports. */
  thumbnailUrl?(id: string): string | undefined;
  /** Where a `<video>` loads a workspace video by preview token (desktop only). */
  mediaUrl?(token: string): string | undefined;
  /**
   * Where the Devices pane streams from (ADR-128): the desktop's loopback proxy, or the
   * remote server's same-origin `/api/device-hub` for a paired device.
   */
  deviceHubAccess?(proxy: DeviceProxy | null): DeviceHubAccess | null;
}
