import { env, fs, readStdin, writeStdout } from '@ortzi/host';
import { UsageError, cli, columns } from '@ortzi/kit';
import { readSecret } from './bao';
import { GarageAdmin, KEY_PREFIX, type Key } from './garage';
import { basename, isRemote, join, parseDuration, parseRemote, type Remote, size } from './path';
import { S3 } from './s3';

const DEFAULT_ENDPOINT = 'https://s3.ortzi.cloud';
const OPENBAO = 'https://vault.ortzi.cloud';
const GARAGE_ADMIN = 'https://garage-admin.ortzi.cloud';
const REGION = 'garage';
const KEY_TTL = 3600;
const KEY_MARGIN = 300;
const CACHE_DIR = '~/.config/ortzi/s3';
const CACHE = `${CACHE_DIR}/key.json`;

const USAGE = `usage: ortzi s3 <command> [flags]

commands:
  ls [s3://BUCKET[/PREFIX]]     list buckets, or the objects under a prefix
  cp SRC DST                    copy between local paths and s3://BUCKET/KEY; '-' is stdin/stdout
  cat s3://BUCKET/KEY           print an object
  rm s3://BUCKET/KEY...         delete objects
  mb s3://BUCKET                make a bucket
  rb s3://BUCKET                remove a bucket
  presign s3://BUCKET/KEY       print a presigned GET URL
  env                           print \`export AWS_...\` lines for aws, rclone and friends
  whoami                        show the key in use

flags:
  --recursive, -r               ls, cp, rm: everything under the prefix or directory
  --force                       rb: delete the bucket's objects first
  --expires DURATION            presign: how long the URL works (default 15m; at most the key's lifetime)
  --refresh                     mint a new key instead of reusing the cached one
  --endpoint URL                S3 endpoint (default: $S3_ENDPOINT or ${DEFAULT_ENDPOINT})
  --token TOKEN                 use this Kanidm token instead of the stored login (default: $S3_TOKEN)
  --json                        ls: print JSON

Each key is minted from your login through OpenBao and Garage's admin API. It lasts
${KEY_TTL / 60} minutes, owns every bucket, and is cached in ${CACHE}.
Local paths must sit under the current directory, and files are held in memory.
log in with \`ortzi login s3\``;

function readCache(): Key | undefined {
  try {
    const key = JSON.parse(fs.readText(CACHE)) as Key;
    return Date.parse(key.expiration) - KEY_MARGIN * 1000 > Date.now() ? key : undefined;
  } catch {
    return undefined;
  }
}

function username(jwt: string): string {
  try {
    const payload = jwt.split('.')[1].replace(/-/g, '+').replace(/_/g, '/');
    const claims = JSON.parse(atob(payload)) as { preferred_username?: string; sub?: string };
    return (claims.preferred_username ?? claims.sub ?? 'user').replace(/[^\w.-]/g, '_');
  } catch {
    return 'user';
  }
}

function mint(jwt: string): Key {
  const { GARAGE_ADMIN_TOKEN: adminToken } = readSecret(OPENBAO, jwt, 'garage');
  if (!adminToken) throw new Error('openbao kv/garage has no GARAGE_ADMIN_TOKEN');

  const admin = new GarageAdmin(GARAGE_ADMIN, adminToken);
  admin.pruneExpired();
  const key = admin.mint(`${KEY_PREFIX}${username(jwt)}-${Date.now()}`, KEY_TTL);

  fs.mkdir(CACHE_DIR);
  fs.write(CACHE, JSON.stringify(key));
  return key;
}

const isDir = (path: string) => {
  try {
    fs.readDir(path);
    return true;
  } catch {
    return false;
  }
};

function walk(dir: string, prefix = ''): string[] {
  return fs.readDir(join(dir, prefix) || '.').flatMap((entry) => {
    const rel = join(prefix, entry.name);
    return entry.dir ? walk(dir, rel) : [rel];
  });
}

function writeLocal(path: string, body: Uint8Array) {
  const parent = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
  if (parent) fs.mkdir(parent);
  fs.write(path, body);
}

const dirPrefix = (key: string) => (!key || key.endsWith('/') ? key : `${key}/`);

const intoDir = (key: string) => !key || key.endsWith('/');

function remote(path: string | undefined, what = 's3://BUCKET/KEY'): Remote {
  if (!path) throw new UsageError(`missing ${what}\n\n${USAGE}`);
  return parseRemote(path);
}

export const run = cli({
  usage: USAGE,
  flags: {
    recursive: { type: 'boolean', short: 'r' },
    force: { type: 'boolean' },
    expires: { type: 'string', default: '15m' },
    refresh: { type: 'boolean' },
    endpoint: { type: 'string' },
    token: { type: 'string' },
    json: { type: 'boolean' },
  },
  setup: (values) => {
    let key: Key | undefined;
    let s3: S3 | undefined;
    const vars = env();
    const endpoint = values.endpoint ?? vars.S3_ENDPOINT ?? DEFAULT_ENDPOINT;

    const credentials = () => {
      if (key) return key;
      if (!values.refresh) key = readCache();
      if (key) return key;

      const jwt = values.token || vars.S3_TOKEN;
      if (!jwt) throw new Error('not logged in; run `ortzi login s3` or set S3_TOKEN');
      return (key = mint(jwt));
    };

    return {
      endpoint,
      credentials,
      s3: () => {
        const { accessKeyId, secretAccessKey } = credentials();
        return (s3 ??= new S3(endpoint, { accessKeyId, secretAccessKey, region: REGION }));
      },
    };
  },
  commands: {
    whoami: (ctx) => {
      const key = ctx.credentials();
      const buckets = ctx.s3().listBuckets();
      console.log(`key       ${key.accessKeyId}`);
      console.log(`expires   ${key.expiration}`);
      console.log(`endpoint  ${ctx.endpoint}`);
      console.log(`buckets   ${buckets.length}`);

      return 0;
    },
    env: (ctx) => {
      const key = ctx.credentials();
      console.log(`export AWS_ACCESS_KEY_ID='${key.accessKeyId}'`);
      console.log(`export AWS_SECRET_ACCESS_KEY='${key.secretAccessKey}'`);
      console.log(`export AWS_ENDPOINT_URL='${ctx.endpoint}'`);
      console.log(`export AWS_REGION='${REGION}'`);

      return 0;
    },
    ls: (ctx, values, [target]) => {
      const s3 = ctx.s3();
      if (!target) {
        const buckets = s3.listBuckets();
        if (values.json) {
          console.log(JSON.stringify(buckets, null, 2));
          return 0;
        }

        console.log(
          columns(
            ['BUCKET', 'CREATED'],
            buckets.map((b) => [b.name, b.created]),
          ),
        );
        return 0;
      }

      const { bucket, key } = parseRemote(target);
      const listing = s3.list(bucket, key, values.recursive ? undefined : '/');
      if (values.json) {
        console.log(JSON.stringify(listing, null, 2));
        return 0;
      }

      const rows = [
        ...listing.prefixes.map((prefix) => ['', '', prefix]),
        ...listing.objects.map((o) => [size(o.size), o.modified, o.key]),
      ];
      console.log(rows.length ? columns(['SIZE', 'MODIFIED', 'KEY'], rows) : 'no objects');
      return 0;
    },
    cp: (ctx, values, [src, dst]) => {
      if (!src || !dst) throw new UsageError(`missing SRC or DST\n\n${USAGE}`);
      if (!isRemote(src) && !isRemote(dst)) throw new UsageError('one side must be s3://');
      const s3 = ctx.s3();

      if (isRemote(src)) {
        const from = parseRemote(src);
        const sources = values.recursive
          ? s3
              .list(from.bucket, dirPrefix(from.key))
              .objects.map((o) => ({ key: o.key, rel: o.key.slice(dirPrefix(from.key).length) }))
          : [{ key: from.key, rel: basename(from.key) }];
        if (!sources.length || !sources[0].key) throw new Error(`nothing to copy at ${src}`);

        for (const { key, rel } of sources) {
          if (isRemote(dst)) {
            const to = parseRemote(dst);
            const target = values.recursive || intoDir(to.key) ? join(to.key, rel) : to.key;
            s3.copy({ bucket: from.bucket, key }, to.bucket, target);
            console.error(`copy: s3://${from.bucket}/${key} -> s3://${to.bucket}/${target}`);
          } else if (dst === '-') {
            writeStdout(s3.get(from.bucket, key));
          } else {
            const target =
              values.recursive || dst.endsWith('/') || isDir(dst) ? join(dst, rel) : dst;
            writeLocal(target, s3.get(from.bucket, key));
            console.error(`download: s3://${from.bucket}/${key} -> ${target}`);
          }
        }
        return 0;
      }

      const to = parseRemote(dst);
      if (values.recursive) {
        for (const rel of walk(src)) {
          const target = join(dirPrefix(to.key), rel);
          s3.put(to.bucket, target, fs.read(join(src, rel)));
          console.error(`upload: ${join(src, rel)} -> s3://${to.bucket}/${target}`);
        }
        return 0;
      }

      if (src === '-' && intoDir(to.key)) throw new UsageError('give a key when copying stdin');
      const target = intoDir(to.key) ? join(to.key, basename(src)) : to.key;
      s3.put(to.bucket, target, src === '-' ? readStdin() : fs.read(src));
      console.error(`upload: ${src} -> s3://${to.bucket}/${target}`);
      return 0;
    },
    cat: (ctx, _values, [path]) => {
      const { bucket, key } = remote(path);
      writeStdout(ctx.s3().get(bucket, key));

      return 0;
    },
    rm: (ctx, values, paths) => {
      if (!paths.length) throw new UsageError(`missing s3://BUCKET/KEY\n\n${USAGE}`);
      const s3 = ctx.s3();

      for (const path of paths) {
        const { bucket, key } = parseRemote(path);
        if (values.recursive) {
          const keys = s3.list(bucket, key).objects.map((o) => o.key);
          s3.deleteMany(bucket, keys);
          console.error(`delete: ${keys.length} objects under s3://${bucket}/${key}`);
        } else {
          if (!key) throw new UsageError(`${path} names a bucket; use rb or rm -r`);
          s3.delete(bucket, key);
          console.error(`delete: s3://${bucket}/${key}`);
        }
      }
      return 0;
    },
    mb: (ctx, _values, [path]) => {
      const { bucket } = remote(path, 's3://BUCKET');
      ctx.s3().createBucket(bucket);
      console.error(`make bucket: ${bucket}`);

      return 0;
    },
    rb: (ctx, values, [path]) => {
      const { bucket } = remote(path, 's3://BUCKET');
      const s3 = ctx.s3();
      if (values.force)
        s3.deleteMany(
          bucket,
          s3.list(bucket, '').objects.map((o) => o.key),
        );

      s3.deleteBucket(bucket);
      console.error(`remove bucket: ${bucket}`);
      return 0;
    },
    presign: (ctx, values, [path]) => {
      const { bucket, key } = remote(path);
      const seconds = parseDuration(values.expires ?? '15m');
      const remaining = Math.floor((Date.parse(ctx.credentials().expiration) - Date.now()) / 1000);
      if (seconds > remaining) {
        throw new UsageError(
          `the key expires in ${Math.floor(remaining / 60)}m and the URL dies with it; use a shorter --expires or --refresh`,
        );
      }

      console.log(ctx.s3().presign(bucket, key, seconds));
      return 0;
    },
  },
});
