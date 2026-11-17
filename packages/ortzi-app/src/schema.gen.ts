import { writeFileSync } from 'node:fs';
import { schemaJson } from './schema.ts';

writeFileSync(new URL('../app.schema.json', import.meta.url), schemaJson());
