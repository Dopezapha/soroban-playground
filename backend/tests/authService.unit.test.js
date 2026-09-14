// Copyright (c) 2026 StellarDevTools
// SPDX-License-Identifier: MIT

import { jest } from '@jest/globals';

jest.mock('jsonwebtoken', () => ({
  __esModule: true,
  default: {
    sign: jest.fn(),
    verify: jest.fn(),
  },
}));

jest.mock('uuid', () => ({
  __esModule: true,
  v4: jest.fn(),
}));

jest.mock('../src/services/redisService.js', () => ({
  __esModule: true,
  default: {
    get: jest.fn(),
    set: jest.fn(),
    del: jest.fn(),
  },
}));

const mockDb = {
  get: jest.fn(),
  all: jest.fn(),
};

jest.mock('../src/database/connection.js', () => ({
  __esModule: true,
  getDatabase: jest.fn(() => mockDb),
}));

jest.mock('../src/services/apiKeyService.js', () => ({
  __esModule: true,
  default: {
    validateKey: jest.fn(),
  },
}));

import jwt from 'jsonwebtoken';
import { v4 as uuidv4 } from 'uuid';
import redisService from '../src/services/redisService.js';
import apiKeyService from '../src/services/apiKeyService.js';
import authService from '../src/services/authService.js';

describe('AuthService', () => {
  afterEach(() => {
    jest.clearAllMocks();
  });

  describe('generateTokens', () => {
    it('should generate access and refresh tokens correctly', async () => {
      uuidv4
        .mockReturnValueOnce('access-jti')
        .mockReturnValueOnce('refresh-jti')
        .mockReturnValueOnce('family-id');

      jwt.sign
        .mockReturnValueOnce('access-token')
        .mockReturnValueOnce('refresh-token');

      const user = { id: 1, username: 'testuser' };
      const result = await authService.generateTokens(user);

      expect(result).toEqual({
        accessToken: 'access-token',
        refreshToken: 'refresh-token',
        accessTokenJti: 'access-jti',
        refreshTokenJti: 'refresh-jti',
        familyId: 'family-id',
      });

      expect(jwt.sign).toHaveBeenCalledTimes(2);
      expect(jwt.sign).toHaveBeenNthCalledWith(
        1,
        {
          sub: user.id,
          username: user.username,
          jti: 'access-jti',
          type: 'access',
        },
        expect.any(String),
        { expiresIn: 15 * 60 }
      );
      expect(jwt.sign).toHaveBeenNthCalledWith(
        2,
        {
          sub: user.id,
          familyId: 'family-id',
          jti: 'refresh-jti',
          type: 'refresh',
        },
        expect.any(String),
        { expiresIn: 7 * 24 * 60 * 60 }
      );
    });
  });

  describe('verifyAccessToken', () => {
    it('should throw if token is blacklisted', async () => {
      const decoded = { jti: 'some-jti', type: 'access' };
      jwt.verify.mockReturnValue(decoded);
      redisService.get.mockResolvedValue('1');

      await expect(authService.verifyAccessToken('token')).rejects.toThrow(
        'Token is blacklisted'
      );
      expect(jwt.verify).toHaveBeenCalledWith('token', expect.any(String));
      expect(redisService.get).toHaveBeenCalledWith('bl_access:some-jti');
    });

    it('should return decoded token if valid and not blacklisted', async () => {
      const decoded = { jti: 'some-jti', type: 'access' };
      jwt.verify.mockReturnValue(decoded);
      redisService.get.mockResolvedValue(null);

      const result = await authService.verifyAccessToken('token');
      expect(result).toEqual(decoded);
    });
  });

  describe('blacklistAccessToken', () => {
    it('should blacklist a token with correct ttl', async () => {
      const now = Math.floor(Date.now() / 1000);
      const exp = now + 3600; // 1 hour later
      await authService.blacklistAccessToken('some-jti', exp);
      expect(redisService.set).toHaveBeenCalledWith(
        'bl_access:some-jti',
        '1',
        expect.any(Number)
      );

      const ttl = redisService.set.mock.calls[0][2];
      expect(ttl).toBeGreaterThan(0);
      expect(ttl).toBeLessThanOrEqual(3600);
    });

    it('should not blacklist if ttl is negative or zero', async () => {
      const now = Math.floor(Date.now() / 1000);
      const exp = now - 100;
      await authService.blacklistAccessToken('some-jti', exp);
      expect(redisService.set).not.toHaveBeenCalled();
    });
  });

  describe('rotateRefreshToken', () => {
    beforeEach(() => {
      uuidv4
        .mockReturnValueOnce('new-access-jti')
        .mockReturnValueOnce('new-refresh-jti');
      jwt.sign
        .mockReturnValueOnce('new-access')
        .mockReturnValueOnce('new-refresh');
    });

    it('should throw if invalid token signature', async () => {
      jwt.verify.mockImplementation(() => {
        throw new Error('invalid signature');
      });
      await expect(authService.rotateRefreshToken('token')).rejects.toThrow(
        'Invalid refresh token'
      );
    });

    it('should throw if wrong token type', async () => {
      jwt.verify.mockReturnValue({ type: 'access' });
      await expect(authService.rotateRefreshToken('token')).rejects.toThrow(
        'Invalid token type'
      );
    });

    it('should detect reuse, invalidate family and throw', async () => {
      const decoded = {
        type: 'refresh',
        jti: 'r-jti',
        familyId: 'f-id',
        exp: Math.floor(Date.now() / 1000) + 1000,
      };
      jwt.verify.mockReturnValue(decoded);

      redisService.get.mockImplementation(async (key) => {
        if (key === 'used_refresh:r-jti') return '1';
        return null;
      });

      await expect(authService.rotateRefreshToken('token')).rejects.toThrow(
        'Refresh token reuse detected. Family invalidated.'
      );
      expect(redisService.set).toHaveBeenCalledWith(
        'bl_family:f-id',
        '1',
        7 * 24 * 60 * 60
      );
    });

    it('should throw if family is blacklisted', async () => {
      const decoded = {
        type: 'refresh',
        jti: 'r-jti',
        familyId: 'f-id',
        exp: Math.floor(Date.now() / 1000) + 1000,
      };
      jwt.verify.mockReturnValue(decoded);
      redisService.get.mockImplementation(async (key) => {
        if (key === 'bl_family:f-id') return '1';
        return null;
      });

      await expect(authService.rotateRefreshToken('token')).rejects.toThrow(
        'Token family is blacklisted due to previous anomaly.'
      );
    });

    it('should rotate successfully', async () => {
      const decoded = {
        sub: 1,
        type: 'refresh',
        jti: 'r-jti',
        familyId: 'f-id',
        exp: Math.floor(Date.now() / 1000) + 1000,
      };
      jwt.verify.mockReturnValue(decoded);
      redisService.get.mockImplementation(async (key) => {
        if (key === 'refresh:r-jti') {
          return JSON.stringify({ sub: 1, familyId: 'f-id' });
        }
        return null;
      });

      const result = await authService.rotateRefreshToken('token');
      expect(result).toEqual({
        accessToken: 'new-access',
        refreshToken: 'new-refresh',
      });
      expect(redisService.set).toHaveBeenCalledWith(
        'used_refresh:r-jti',
        '1',
        expect.any(Number)
      );
      expect(jwt.sign).toHaveBeenCalledTimes(2);
    });
  });

  describe('getUserById', () => {
    it('should return null if no userId', async () => {
      expect(await authService.getUserById()).toBeNull();
    });

    it('should return user object if found', async () => {
      const user = {
        id: 1,
        username: 'test',
        email: 'test@e.com',
        role: 'admin',
      };
      mockDb.get.mockResolvedValue(user);
      const result = await authService.getUserById(1);
      expect(result).toEqual(user);
      expect(mockDb.get).toHaveBeenCalledWith(
        'SELECT id, username, email, role FROM users WHERE id = ?',
        [1]
      );
    });
  });

  describe('getUserPermissions', () => {
    it('should return empty array if no userId', async () => {
      expect(await authService.getUserPermissions()).toEqual([]);
    });

    it('should return array of permission names', async () => {
      mockDb.all.mockResolvedValue([{ name: 'read' }, { name: 'write' }]);
      const result = await authService.getUserPermissions(1);
      expect(result).toEqual(['read', 'write']);
      expect(mockDb.all).toHaveBeenCalled();
    });
  });

  describe('authenticate', () => {
    afterEach(() => {
      jest.spyOn(authService, 'getUserById').mockRestore();
      jest.spyOn(authService, 'getUserPermissions').mockRestore();
    });

    it('should authenticate via API Key Bearer token', async () => {
      const req = { headers: { authorization: 'Bearer my-token' } };
      apiKeyService.validateKey.mockResolvedValue({ userId: 10 });
      jest
        .spyOn(authService, 'getUserById')
        .mockResolvedValue({ id: 10, role: 'user' });
      jest
        .spyOn(authService, 'getUserPermissions')
        .mockResolvedValue(['read:data']);

      const user = await authService.authenticate(req);
      expect(user).toEqual({
        id: 10,
        role: 'user',
        permissions: ['read:data'],
      });
    });

    it('should fallback to session if no valid API key', async () => {
      const req = { headers: {}, session: { userId: 20 } };
      jest
        .spyOn(authService, 'getUserById')
        .mockResolvedValue({ id: 20, role: 'admin' });
      jest.spyOn(authService, 'getUserPermissions').mockResolvedValue(['all']);

      const user = await authService.authenticate(req);
      expect(user).toEqual({ id: 20, role: 'admin', permissions: ['all'] });
    });

    it('should fallback to x-user-id header in non-production', async () => {
      const req = { headers: { 'x-user-id': '30' } };
      jest
        .spyOn(authService, 'getUserById')
        .mockResolvedValue({ id: 30, role: 'user' });
      jest.spyOn(authService, 'getUserPermissions').mockResolvedValue(['read']);

      const user = await authService.authenticate(req);
      expect(user).toEqual({ id: 30, role: 'user', permissions: ['read'] });
    });

    it('should default to guest if nothing matches', async () => {
      const req = { headers: {} };
      const user = await authService.authenticate(req);
      expect(user).toEqual({
        id: null,
        username: 'anonymous',
        email: '',
        role: 'guest',
        permissions: ['project:read'],
      });
    });
  });

  describe('hasPermission', () => {
    it('returns false if no user', () => {
      expect(authService.hasPermission(null, 'read')).toBe(false);
    });

    it('returns true if user is admin', () => {
      expect(authService.hasPermission({ role: 'admin' }, 'read')).toBe(true);
    });

    it('returns true if user has permission', () => {
      expect(
        authService.hasPermission(
          { role: 'user', permissions: ['read'] },
          'read'
        )
      ).toBe(true);
    });

    it('returns false if user does not have permission', () => {
      expect(
        authService.hasPermission(
          { role: 'user', permissions: ['read'] },
          'write'
        )
      ).toBe(false);
    });
  });

  describe('hasRole', () => {
    it('returns false if no user', () => {
      expect(authService.hasRole(null, 'admin')).toBe(false);
    });

    it('returns true if user has the single role', () => {
      expect(authService.hasRole({ role: 'admin' }, 'admin')).toBe(true);
    });

    it('returns true if user role is in array of roles', () => {
      expect(authService.hasRole({ role: 'user' }, ['admin', 'user'])).toBe(
        true
      );
    });

    it('returns false if user role is not in array', () => {
      expect(authService.hasRole({ role: 'guest' }, ['admin', 'user'])).toBe(
        false
      );
    });
  });
});
