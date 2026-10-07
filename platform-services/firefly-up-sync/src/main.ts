import { Config } from "./config.ts";
import { FireflyClient } from "./firefly.ts";
import { Sync } from "./sync.ts";
import { UpClient } from "./up.ts";

try {
  const config = new Config(process.env);
  const up = new UpClient(config.upToken);
  const firefly = new FireflyClient(config.fireflyUrl, config.fireflyToken);

  const result = await new Sync(config, up, firefly).run(new Date());

  console.log(
    `done: fetched ${result.fetched} from Up, ${result.alreadyImported} already in Firefly, created ${result.created}` +
      (config.dryRun ? " (dry run)" : ""),
  );
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
}
