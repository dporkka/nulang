/* Minimal C example of embedding Nulang through the stable C API. */
#include <stdio.h>
#include <stdint.h>
#include <string.h>

/* Opaque runtime handle. */
typedef struct NulangRuntime NulangRuntime;

/* Nulang value passed by value (raw NaN-boxed bits). */
typedef struct {
    uint64_t raw;
} NulangValue;

NulangRuntime *nulang_runtime_new(void);
void nulang_runtime_free(NulangRuntime *runtime);
int64_t nulang_compile(NulangRuntime *runtime, const char *source);
NulangValue nulang_run(NulangRuntime *runtime, int64_t module_handle);
const char *nulang_last_error(NulangRuntime *runtime);
int64_t nulang_value_int(NulangValue value);
double nulang_value_float(NulangValue value);
const char *nulang_value_to_string(NulangRuntime *runtime, NulangValue value);

int main(void) {
    NulangRuntime *rt = nulang_runtime_new();
    if (!rt) {
        fprintf(stderr, "failed to create runtime\n");
        return 1;
    }

    const char *source =
        "extern \"libm.so.6\" {\n"
        "  fn sqrt(x: Float) -> Float\n"
        "}\n"
        "sqrt(9.0)\n";

    int64_t handle = nulang_compile(rt, source);
    if (handle < 0) {
        fprintf(stderr, "compile error: %s\n", nulang_last_error(rt));
        nulang_runtime_free(rt);
        return 1;
    }

    NulangValue result = nulang_run(rt, handle);
    const char *err = nulang_last_error(rt);
    if (err && strlen(err) > 0) {
        fprintf(stderr, "run error: %s\n", err);
        nulang_runtime_free(rt);
        return 1;
    }

    double f = nulang_value_float(result);
    printf("sqrt(9.0) = %f\n", f);

    const char *repr = nulang_value_to_string(rt, result);
    printf("string repr: %s\n", repr ? repr : "(null)");

    nulang_runtime_free(rt);
    return 0;
}
