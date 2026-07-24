use std::sync::Once;

static VEC_INIT: Once = Once::new();

pub fn register() {
    VEC_INIT.call_once(|| unsafe {
        libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut libsqlite3_sys::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const libsqlite3_sys::sqlite3_api_routines,
            ) -> std::os::raw::c_int,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    });
}
