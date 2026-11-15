import { expect } from "@playwright/test";

export function formatDefaultUtcTimestamp(rfc3339) {
  const timestamp = new Date(rfc3339);
  expect(timestamp.valueOf()).not.toBeNaN();
  const date = timestamp.toLocaleDateString("en-US", {
    timeZone: "UTC",
    month: "short",
    day: "2-digit",
    year: "numeric",
  });
  return `${date} ${formatDefaultUtcTime(rfc3339)}`;
}

export function formatDefaultUtcTime(rfc3339) {
  return new Date(rfc3339).toLocaleTimeString("en-US", {
    timeZone: "UTC",
    hour: "numeric",
    minute: "2-digit",
    hour12: true,
    timeZoneName: "short",
  });
}
