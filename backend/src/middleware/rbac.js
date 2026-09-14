// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT
//
// Multi-Tenant Organization Workspaces & RBAC Permission Matrix
// Implements role-based access control for team contract deployments
// and shared API keys within organization workspaces.
//
// Roles (least → most privileged):
//   viewer   – read-only access to org resources
//   deployer – viewer + deploy contracts
//   manager  – deployer + manage team members and API keys
//   admin    – full control, including org settings and billing
//   owner    – super-admin; can transfer/delete the organization
//
// Permissions are additive: each role inherits all permissions of roles below it.

import { createHttpError } from './errorHandler.js';

// ── Permission definitions ─────────────────────────────────────────────────────

/** All defined permissions in the system. */
export const PERMISSIONS = Object.freeze({
  // Contract operations
  CONTRACT_READ: 'contract:read',
  CONTRACT_DEPLOY: 'contract:deploy',
  CONTRACT_INVOKE: 'contract:invoke',
  CONTRACT_DELETE: 'contract:delete',

  // API key operations
  API_KEY_READ: 'apikey:read',
  API_KEY_CREATE: 'apikey:create',
  API_KEY_REVOKE: 'apikey:revoke',

  // Workspace / team management
  MEMBER_READ: 'member:read',
  MEMBER_INVITE: 'member:invite',
  MEMBER_REMOVE: 'member:remove',
  MEMBER_ROLE_ASSIGN: 'member:role_assign',

  // Organization settings
  ORG_READ: 'org:read',
  ORG_UPDATE: 'org:update',
  ORG_DELETE: 'org:delete',
  ORG_TRANSFER: 'org:transfer',

  // Webhook subscriptions
  WEBHOOK_READ: 'webhook:read',
  WEBHOOK_MANAGE: 'webhook:manage',

  // Billing / usage
  BILLING_READ: 'billing:read',
  BILLING_MANAGE: 'billing:manage',
});

// ── Role → permission matrix ───────────────────────────────────────────────────

/** Map each role to the full set of permissions it grants (cumulative). */
export const ROLE_PERMISSIONS = Object.freeze({
  viewer: new Set([
    PERMISSIONS.CONTRACT_READ,
    PERMISSIONS.API_KEY_READ,
    PERMISSIONS.MEMBER_READ,
    PERMISSIONS.ORG_READ,
    PERMISSIONS.WEBHOOK_READ,
    PERMISSIONS.BILLING_READ,
  ]),

  deployer: new Set([
    PERMISSIONS.CONTRACT_READ,
    PERMISSIONS.CONTRACT_DEPLOY,
    PERMISSIONS.CONTRACT_INVOKE,
    PERMISSIONS.API_KEY_READ,
    PERMISSIONS.MEMBER_READ,
    PERMISSIONS.ORG_READ,
    PERMISSIONS.WEBHOOK_READ,
    PERMISSIONS.BILLING_READ,
  ]),

  manager: new Set([
    PERMISSIONS.CONTRACT_READ,
    PERMISSIONS.CONTRACT_DEPLOY,
    PERMISSIONS.CONTRACT_INVOKE,
    PERMISSIONS.CONTRACT_DELETE,
    PERMISSIONS.API_KEY_READ,
    PERMISSIONS.API_KEY_CREATE,
    PERMISSIONS.API_KEY_REVOKE,
    PERMISSIONS.MEMBER_READ,
    PERMISSIONS.MEMBER_INVITE,
    PERMISSIONS.MEMBER_REMOVE,
    PERMISSIONS.MEMBER_ROLE_ASSIGN,
    PERMISSIONS.ORG_READ,
    PERMISSIONS.WEBHOOK_READ,
    PERMISSIONS.WEBHOOK_MANAGE,
    PERMISSIONS.BILLING_READ,
  ]),

  admin: new Set([
    PERMISSIONS.CONTRACT_READ,
    PERMISSIONS.CONTRACT_DEPLOY,
    PERMISSIONS.CONTRACT_INVOKE,
    PERMISSIONS.CONTRACT_DELETE,
    PERMISSIONS.API_KEY_READ,
    PERMISSIONS.API_KEY_CREATE,
    PERMISSIONS.API_KEY_REVOKE,
    PERMISSIONS.MEMBER_READ,
    PERMISSIONS.MEMBER_INVITE,
    PERMISSIONS.MEMBER_REMOVE,
    PERMISSIONS.MEMBER_ROLE_ASSIGN,
    PERMISSIONS.ORG_READ,
    PERMISSIONS.ORG_UPDATE,
    PERMISSIONS.WEBHOOK_READ,
    PERMISSIONS.WEBHOOK_MANAGE,
    PERMISSIONS.BILLING_READ,
    PERMISSIONS.BILLING_MANAGE,
  ]),

  owner: new Set([
    ...Object.values(PERMISSIONS), // owns everything
  ]),
});

/** Ordered list of roles from least to most privileged. */
export const ROLE_HIERARCHY = [
  'viewer',
  'deployer',
  'manager',
  'admin',
  'owner',
];

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Returns true when `role` is a valid, known RBAC role.
 * @param {string} role
 * @returns {boolean}
 */
export function isValidRole(role) {
  return Object.prototype.hasOwnProperty.call(ROLE_PERMISSIONS, role);
}

/**
 * Returns the complete set of permissions granted to a role (or an empty set
 * for unrecognised roles).
 * @param {string} role
 * @returns {Set<string>}
 */
export function getPermissionsForRole(role) {
  return ROLE_PERMISSIONS[role] ?? new Set();
}

/**
 * Returns true when `targetRole` is at least as privileged as `minimumRole`.
 * @param {string} targetRole
 * @param {string} minimumRole
 * @returns {boolean}
 */
export function roleAtLeast(targetRole, minimumRole) {
  const targetIdx = ROLE_HIERARCHY.indexOf(targetRole);
  const minIdx = ROLE_HIERARCHY.indexOf(minimumRole);
  if (targetIdx === -1 || minIdx === -1) return false;
  return targetIdx >= minIdx;
}

/**
 * Returns true when the principal (req.auth or req.user) has the given
 * permission in the current org/tenant context.
 *
 * Precedence order for role resolution:
 *   1. req.auth.roles[]   – roles attached to the current API key / JWT
 *   2. req.user.role      – legacy single-role field from authMiddleware
 *
 * @param {import('express').Request} req
 * @param {string} permission
 * @returns {boolean}
 */
export function hasPermission(req, permission) {
  const roles = resolveRoles(req);
  return roles.some((role) => {
    const perms = getPermissionsForRole(role);
    return perms.has(permission);
  });
}

/**
 * Returns true when the principal holds at least one of the listed roles,
 * or holds a role that is higher in the hierarchy than any listed role.
 *
 * @param {import('express').Request} req
 * @param {string|string[]} roles
 * @returns {boolean}
 */
export function hasRole(req, roles) {
  const required = Array.isArray(roles) ? roles : [roles];
  const principalRoles = resolveRoles(req);
  return principalRoles.some((pr) =>
    required.some((rr) => roleAtLeast(pr, rr))
  );
}

/**
 * Resolves the effective roles for the current request principal.
 * Normalises across both the new multi-role (req.auth.roles) and the legacy
 * single-role (req.user.role) conventions.
 *
 * @param {import('express').Request} req
 * @returns {string[]}
 */
function resolveRoles(req) {
  const roles = [];

  // New-style: array of roles on the auth context (API key / JWT)
  if (Array.isArray(req.auth?.roles)) {
    roles.push(...req.auth.roles);
  }

  // Legacy single-role field from authMiddleware / authService
  if (req.user?.role && typeof req.user.role === 'string') {
    roles.push(req.user.role);
  }

  // Deduplicate
  return [...new Set(roles)];
}

// ── Express middleware factories ───────────────────────────────────────────────

/**
 * Middleware: reject requests whose principal lacks the specified permission.
 *
 * @param {string} permission - One of the PERMISSIONS constants.
 * @returns {import('express').RequestHandler}
 *
 * @example
 * router.post('/deploy', requirePermission(PERMISSIONS.CONTRACT_DEPLOY), handler);
 */
export function requirePermission(permission) {
  return (req, _res, next) => {
    if (!req.auth && !req.user) {
      return next(createHttpError(401, 'Authentication required'));
    }
    if (!hasPermission(req, permission)) {
      return next(
        createHttpError(
          403,
          `Forbidden: permission "${permission}" is required`
        )
      );
    }
    return next();
  };
}

/**
 * Middleware: reject requests whose principal does not hold at least one of
 * the required roles (or a higher-privileged role).
 *
 * @param {string|string[]} roles - One or more role names.
 * @returns {import('express').RequestHandler}
 *
 * @example
 * router.delete('/org', requireRole('owner'), handler);
 * router.post('/members', requireRole(['manager', 'admin', 'owner']), handler);
 */
export function requireRole(roles) {
  const required = Array.isArray(roles) ? roles : [roles];
  return (req, _res, next) => {
    if (!req.auth && !req.user) {
      return next(createHttpError(401, 'Authentication required'));
    }
    if (!hasRole(req, required)) {
      return next(
        createHttpError(
          403,
          `Forbidden: one of the following roles is required: ${required.join(', ')}`
        )
      );
    }
    return next();
  };
}

/**
 * Middleware: enforce that the request is scoped to a specific organization.
 * Compares `req.tenant.id` against the `orgId` route parameter (or a custom
 * param name) and rejects mismatches with 403 to prevent cross-org data leaks.
 *
 * @param {object}  [options]
 * @param {string}  [options.param='orgId'] - Express route param holding the org ID.
 * @param {boolean} [options.allowAdmin=true] - When true, requests carrying the
 *   "admin" or "owner" role bypass the tenant-match check (useful for internal
 *   admin tooling).
 * @returns {import('express').RequestHandler}
 */
export function requireOrgAccess({ param = 'orgId', allowAdmin = true } = {}) {
  return (req, _res, next) => {
    if (!req.tenant?.id) {
      return next(createHttpError(401, 'Tenant context is required'));
    }

    // Super-admins may access any org
    if (allowAdmin && hasRole(req, ['admin', 'owner'])) {
      return next();
    }

    const routeOrgId = req.params?.[param];
    if (routeOrgId && routeOrgId !== req.tenant.id) {
      // Return 404 instead of 403 to avoid org ID enumeration
      return next(createHttpError(404, 'Organization not found'));
    }

    return next();
  };
}

/**
 * Middleware: restrict access to requests that carry a valid organization API
 * key associated with the workspace. Intended for service-to-service calls
 * where a JWT may not be present.
 *
 * Sets `req.auth.organizationId` from the resolved API key metadata so
 * downstream middleware can rely on it.
 *
 * @param {string|string[]} [requiredPermissions] - Optional permission(s) the
 *   API key must hold. Falls back to just validating the key if omitted.
 * @returns {import('express').RequestHandler}
 */
export function requireOrgApiKey(requiredPermissions = []) {
  const required = Array.isArray(requiredPermissions)
    ? requiredPermissions
    : [requiredPermissions];

  return (req, _res, next) => {
    // tenantContext middleware should already have resolved the API key into
    // req.auth.  If req.auth.apiKeyId is present the key was validated.
    if (!req.auth?.apiKeyId) {
      return next(
        createHttpError(
          401,
          'A valid organization API key is required for this endpoint'
        )
      );
    }

    if (!req.auth.organizationId) {
      return next(
        createHttpError(
          403,
          'API key is not associated with an organization workspace'
        )
      );
    }

    if (required.length > 0) {
      const missing = required.filter((p) => !hasPermission(req, p));
      if (missing.length > 0) {
        return next(
          createHttpError(
            403,
            `API key is missing required permissions: ${missing.join(', ')}`
          )
        );
      }
    }

    return next();
  };
}

export default {
  PERMISSIONS,
  ROLE_PERMISSIONS,
  ROLE_HIERARCHY,
  isValidRole,
  getPermissionsForRole,
  roleAtLeast,
  hasPermission,
  hasRole,
  requirePermission,
  requireRole,
  requireOrgAccess,
  requireOrgApiKey,
};
