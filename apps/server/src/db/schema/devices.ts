import { sql } from "drizzle-orm";
import { jsonb, pgTable, text, timestamp, unique, uuid } from "drizzle-orm/pg-core";
import { users } from "./auth";

export const devices = pgTable(
  "devices",
  {
    id: uuid()
      .primaryKey()
      .default(sql`uuidv7()`),
    userId: text()
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    installationId: uuid().notNull(),
    name: text().notNull(),
    platform: text().notNull(),
    // APNs token; a device gets pushes only while one of its sessions is signed in.
    pushToken: text(),
    createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
  },
  (table) => [unique().on(table.userId, table.installationId)],
);

// A call lives only while its caller waits: the caller deletes it when the answer arrives or when it
// gives up, so a device that connects later never finds an old call to run.
export const deviceCalls = pgTable("device_calls", {
  id: uuid()
    .primaryKey()
    .default(sql`uuidv7()`),
  deviceId: uuid()
    .notNull()
    .references(() => devices.id, { onDelete: "cascade" }),
  method: text().notNull(),
  params: jsonb().$type<unknown>().notNull(),
  pickedAt: timestamp({ withTimezone: true }),
  response: jsonb().$type<unknown>(),
  createdAt: timestamp({ withTimezone: true }).notNull().defaultNow(),
});
