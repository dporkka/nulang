export interface Env {
  BUCKET: R2Bucket;
  PUBLISH_TOKEN: string;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;
    const method = request.method;

    // Reject bad path characters to prevent directory traversal
    if (path.includes('..')) {
      return new Response('Bad Request', { status: 400 });
    }

    // Pattern: /api/v1/packages/:name/:version
    const matchVersion = path.match(/^\/api\/v1\/packages\/([^\/]+)\/([^\/]+)$/);
    if (matchVersion) {
      const name = matchVersion[1];
      const version = matchVersion[2];
      const key = `${name}/${version}.tar.gz`;

      if (method === 'GET') {
        const object = await env.BUCKET.get(key);
        if (!object) {
          return new Response('Not found', { status: 404 });
        }
        
        const headers = new Headers();
        object.writeHttpMetadata(headers);
        headers.set('Content-Type', 'application/octet-stream');

        return new Response(object.body as ReadableStream, {
          headers
        });
      }

      if (method === 'PUT') {
        const auth = request.headers.get('Authorization');
        if (!env.PUBLISH_TOKEN || auth !== `Bearer ${env.PUBLISH_TOKEN}`) {
          return new Response('Unauthorized', { status: 401 });
        }
        
        // Check if version already exists
        const existing = await env.BUCKET.head(key);
        if (existing) {
          return new Response('Conflict: Version already exists', { status: 409 });
        }
        
        await env.BUCKET.put(key, request.body);
        return new Response('Created', { status: 201 });
      }
    }

    // Pattern: /api/v1/packages/:name
    const matchName = path.match(/^\/api\/v1\/packages\/([^\/]+)$/);
    if (matchName && method === 'GET') {
      const name = matchName[1];
      const prefix = `${name}/`;
      
      const listed = await env.BUCKET.list({ prefix });
      const versions = listed.objects.map(obj => {
        // key format: "name/version.tar.gz" -> extract "version"
        return obj.key.substring(prefix.length).replace('.tar.gz', '');
      });

      if (versions.length === 0) {
        return new Response('Not found', { status: 404 });
      }

      return new Response(JSON.stringify({ name, versions }), {
        headers: { 'Content-Type': 'application/json' }
      });
    }

    return new Response('Not found', { status: 404 });
  }
}
