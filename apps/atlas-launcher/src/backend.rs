//! The launcher's QObject for QML. Never blocks the GUI thread: work runs on
//! the search worker and comes back with `qt_thread().queue(..)`.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qproperty(QString, query)]
        #[namespace = "atlas_launcher"]
        type Backend = super::BackendRust;
    }

    impl cxx_qt::Threading for Backend {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn backend_make_unique() -> UniquePtr<Backend>;
    }
}

use cxx_qt_lib::QString;

#[derive(Default)]
pub struct BackendRust {
    query: QString,
}
