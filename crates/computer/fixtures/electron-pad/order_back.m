// Orders an Electron window behind every other window, the way the other fixtures order theirs:
// Electron's own showInactive() orders it to the front. A Node-API addon with the few calls it
// needs declared here, so it builds with clang alone.
//
//   clang -bundle -undefined dynamic_lookup -fobjc-arc -framework AppKit order_back.m -o order_back.node

#import <AppKit/AppKit.h>

typedef struct napi_env__ *napi_env;
typedef struct napi_value__ *napi_value;
typedef struct napi_callback_info__ *napi_callback_info;
typedef int napi_status;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);
napi_status napi_create_function(napi_env, const char *, size_t, napi_callback, void *, napi_value *);
napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t *, napi_value *, napi_value *, void **);
napi_status napi_get_buffer_info(napi_env, napi_value, void **, size_t *);
napi_status napi_set_named_property(napi_env, napi_value, const char *, napi_value);
napi_status napi_get_boolean(napi_env, bool, napi_value *);

// orderBack(handle: Buffer): handle is BrowserWindow.getNativeWindowHandle(), an NSView pointer.
static napi_value orderBack(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_value result;
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  void *data = NULL;
  size_t len = 0;
  bool ok = false;
  if (argc == 1 && napi_get_buffer_info(env, argv[0], &data, &len) == 0 && len >= sizeof(void *)) {
    NSView *view = (__bridge NSView *)(*(void **)data);
    NSWindow *window = view.window;
    if (window) {
      [window orderBack:nil];
      ok = true;
    }
  }
  napi_get_boolean(env, ok, &result);
  return result;
}

__attribute__((visibility("default"))) int32_t node_api_module_get_api_version_v1(void) { return 8; }

__attribute__((visibility("default"))) napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value fn;
  napi_create_function(env, "orderBack", (size_t)-1, orderBack, NULL, &fn);
  napi_set_named_property(env, exports, "orderBack", fn);
  return exports;
}
