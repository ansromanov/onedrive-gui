cmake_minimum_required(VERSION 3.21)
project(onedrive-gui-dolphin LANGUAGES CXX)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core DBus)
find_package(KF6KIO REQUIRED)
find_package(KF6CoreAddons REQUIRED)
set(CMAKE_AUTOMOC ON)

add_library(onedrivegui-overlay MODULE overlay.cpp)
target_link_libraries(onedrivegui-overlay PRIVATE Qt6::DBus KF6::KIOCore KF6::CoreAddons)
set_target_properties(onedrivegui-overlay PROPERTIES PREFIX "")

include(GNUInstallDirs)
install(TARGETS onedrivegui-overlay DESTINATION ${CMAKE_INSTALL_LIBDIR}/qt6/plugins/kf6/overlayicon)

option(BUILD_TESTING "Build the overlay check tool" OFF)
if(BUILD_TESTING)
    add_executable(overlay-check tests/overlay-check.cpp)
    target_link_libraries(overlay-check PRIVATE KF6::KIOCore KF6::CoreAddons)
endif()
