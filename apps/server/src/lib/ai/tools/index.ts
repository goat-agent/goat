import { computerTools } from "./computer";
import { connectTools } from "./connect";
import { fileTools } from "./files";
import { historyTools } from "./history";
import { memoryTools } from "./memory";
import { scheduleTools } from "./schedules";
import { searchTools } from "./search";
import { timeTools } from "./time";
import { webTools } from "./web";

export const tools = {
  ...webTools,
  ...timeTools,
  ...searchTools,
  ...connectTools,
  ...fileTools,
  ...computerTools,
  ...scheduleTools,
  ...memoryTools,
  ...historyTools,
};
