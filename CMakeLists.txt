cmake_minimum_required(VERSION 3.20)

project(clipbus VERSION 0.1.0 LANGUAGES C)

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)

set(CLIPBUS_CARGO_PROFILE release CACHE STRING "Cargo profile used for install builds")

if(CLIPBUS_CARGO_PROFILE STREQUAL "release")
    set(CLIPBUS_CARGO_FLAGS --release)
    set(CLIPBUS_CARGO_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/release")
else()
    set(CLIPBUS_CARGO_FLAGS)
    set(CLIPBUS_CARGO_TARGET_DIR "${CMAKE_CURRENT_SOURCE_DIR}/target/debug")
endif()

if(CMAKE_STATIC_LIBRARY_SUFFIX)
    set(CLIPBUS_STATIC_LIBRARY
        "${CLIPBUS_CARGO_TARGET_DIR}/${CMAKE_STATIC_LIBRARY_PREFIX}clipbus${CMAKE_STATIC_LIBRARY_SUFFIX}")
else()
    set(CLIPBUS_STATIC_LIBRARY "${CLIPBUS_CARGO_TARGET_DIR}/libclipbus.a")
endif()

add_custom_command(
    OUTPUT "${CLIPBUS_STATIC_LIBRARY}"
    COMMAND cargo build ${CLIPBUS_CARGO_FLAGS}
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    DEPENDS
        "${CMAKE_CURRENT_SOURCE_DIR}/Cargo.toml"
        "${CMAKE_CURRENT_SOURCE_DIR}/src/lib.rs"
        "${CMAKE_CURRENT_SOURCE_DIR}/src/abi.rs"
        "${CMAKE_CURRENT_SOURCE_DIR}/src/clipboard.rs"
        "${CMAKE_CURRENT_SOURCE_DIR}/src/x11.rs"
    VERBATIM)

add_custom_target(clipbus_cargo_build ALL DEPENDS "${CLIPBUS_STATIC_LIBRARY}")

add_library(clipbus STATIC IMPORTED GLOBAL)
add_dependencies(clipbus clipbus_cargo_build)
set_target_properties(clipbus PROPERTIES
    IMPORTED_LOCATION "${CLIPBUS_STATIC_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/include")

add_library(Clipbus::Clipbus ALIAS clipbus)

install(FILES include/clipbus.h DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}")
install(FILES "${CLIPBUS_STATIC_LIBRARY}" DESTINATION "${CMAKE_INSTALL_LIBDIR}")

set(prefix "${CMAKE_INSTALL_PREFIX}")
set(version "${PROJECT_VERSION}")
configure_file(
    pkgconfig/clipbus.pc.in
    "${CMAKE_CURRENT_BINARY_DIR}/clipbus.pc"
    @ONLY)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/clipbus.pc"
        DESTINATION "${CMAKE_INSTALL_LIBDIR}/pkgconfig")

configure_package_config_file(
    cmake/ClipbusConfig.cmake.in
    "${CMAKE_CURRENT_BINARY_DIR}/ClipbusConfig.cmake"
    INSTALL_DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/Clipbus")
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/ClipbusConfig.cmake"
        DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/Clipbus")
