import { deprecationHeaders } from '../src/middleware/deprecationHeaders.js';

describe('deprecationHeaders middleware', () => {
  let req;
  let res;
  let next;
  let headers;

  beforeEach(() => {
    headers = {};
    req = {};
    res = {
      setHeader: jest.fn((name, value) => {
        headers[name] = value;
      }),
    };
    next = jest.fn();
  });

  it('sets Deprecation, Sunset, and Link headers for deprecated v1', () => {
    req.apiVersion = 'v1';
    deprecationHeaders(req, res, next);

    expect(res.setHeader).toHaveBeenCalledWith('API-Version', 'v1');
    expect(res.setHeader).toHaveBeenCalledWith(
      'Deprecation',
      expect.any(String)
    );
    expect(res.setHeader).toHaveBeenCalledWith('Sunset', expect.any(String));
    expect(headers['Link']).toBeDefined();
    expect(next).toHaveBeenCalledTimes(1);
  });

  it('sets API-Version but not Deprecation headers for active v2', () => {
    req.apiVersion = 'v2';
    deprecationHeaders(req, res, next);

    expect(res.setHeader).toHaveBeenCalledWith('API-Version', 'v2');
    expect(res.setHeader).not.toHaveBeenCalledWith(
      'Deprecation',
      expect.anything()
    );
    expect(res.setHeader).not.toHaveBeenCalledWith('Sunset', expect.anything());
    expect(next).toHaveBeenCalledTimes(1);
  });

  it('calls next without headers if version is unknown', () => {
    req.apiVersion = 'unknown-version';
    deprecationHeaders(req, res, next);

    expect(res.setHeader).not.toHaveBeenCalled();
    expect(next).toHaveBeenCalledTimes(1);
  });
});
