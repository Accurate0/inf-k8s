import { readFileSync } from 'node:fs';
import {
  buildSchema,
  type ConstDirectiveNode,
  GraphQLError,
  type GraphQLFieldResolver,
  GraphQLInterfaceType,
  GraphQLObjectType,
  type GraphQLSchema,
  GraphQLUnionType,
  Kind,
} from 'graphql';
import type { Principal } from './auth.ts';

export const SUBGRAPH_SDL = readFileSync(new URL('../subgraph.graphql', import.meta.url), 'utf8');

// what composition and the router add around the committed sdl
const FEDERATION = `
directive @link(url: String!, import: [String]) repeatable on SCHEMA
directive @key(fields: String!, resolvable: Boolean = true) repeatable on OBJECT | INTERFACE
directive @requiresScopes(scopes: [[String!]!]!) on FIELD_DEFINITION | OBJECT | INTERFACE | SCALAR | ENUM
scalar _Any
type _Service { sdl: String! }
union _Entity = FluxApp | ForgejoCommit | ForgejoRun
extend type Query {
  _entities(representations: [_Any!]!): [_Entity]!
  _service: _Service!
}
`;

export interface Context {
  principal: Principal;
}

export type Resolvers = Record<string, Record<string, GraphQLFieldResolver<any, Context, any>>>;

function alternatives(directives: readonly ConstDirectiveNode[] | undefined): string[][] {
  const directive = directives?.find((d) => d.name.value === 'requiresScopes');
  const argument = directive?.arguments?.find((a) => a.name.value === 'scopes');
  if (argument?.value.kind !== Kind.LIST) {
    return [];
  }
  return argument.value.values.map((group) =>
    group.kind === Kind.LIST
      ? group.values.flatMap((scope) => (scope.kind === Kind.STRING ? [scope.value] : []))
      : [],
  );
}

const satisfied = (required: string[][], held: Set<string>) =>
  !required.length || required.some((all) => all.every((scope) => held.has(scope)));

export function missingScope(required: string[][]): GraphQLError {
  return new GraphQLError(`missing scope ${required.map((all) => all.join(' ')).join(' or ')}`, {
    extensions: { code: 'FORBIDDEN' },
  });
}

const typename = (value: { __typename?: string }) => value.__typename;

export function executableSchema(resolvers: Resolvers): GraphQLSchema {
  const schema = buildSchema(`${SUBGRAPH_SDL}\n${FEDERATION}`);

  for (const type of Object.values(schema.getTypeMap())) {
    if (type instanceof GraphQLInterfaceType || type instanceof GraphQLUnionType) {
      type.resolveType = typename;
      continue;
    }
    if (!(type instanceof GraphQLObjectType) || type.name.startsWith('__')) {
      continue;
    }

    const typeScopes = alternatives(type.astNode?.directives);
    for (const field of Object.values(type.getFields())) {
      const required = [typeScopes, alternatives(field.astNode?.directives)].filter(
        (list) => list.length,
      );
      const resolve = resolvers[type.name]?.[field.name] ?? field.resolve;
      field.resolve = (source, args, context: Context, info) => {
        for (const list of required) {
          if (!satisfied(list, context.principal.scopes)) {
            throw missingScope(list);
          }
        }
        return resolve
          ? resolve(source, args, context, info)
          : (source as Record<string, unknown>)?.[field.name];
      };
    }
  }

  return schema;
}
