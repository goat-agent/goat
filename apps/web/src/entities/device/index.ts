import { queryOptions, useQuery } from "@tanstack/react-query";
import { api, unwrap, type Device } from "@/shared/api/client";

export { DeviceIcon } from "./device-icon";

export const devicesQuery = queryOptions({
  queryKey: ["devices"],
  queryFn: async () => (await unwrap(api.GET("/devices"))).devices,
});

export function useDevices(): Device[] {
  return useQuery(devicesQuery).data ?? [];
}
