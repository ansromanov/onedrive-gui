cmake_minimum_required(VERSION 3.21)
project(onedrive-gui LANGUAGES CXX)

find_package(Qt6 6.5 REQUIRED COMPONENTS Widgets Quick DBus)
find_package(LayerShellQt QUIET)
qt_standard_project_setup(REQUIRES 6.5)

qt_add_executable(onedrive-gui main.cpp)
qt_add_qml_module(onedrive-gui URI OneDriveGui QML_FILES Main.qml Settings.qml)
target_link_libraries(onedrive-gui PRIVATE Qt6::Widgets Qt6::Quick Qt6::DBus)

if(LayerShellQt_FOUND)
    target_link_libraries(onedrive-gui PRIVATE LayerShellQt::Interface)
    target_compile_definitions(onedrive-gui PRIVATE HAVE_LAYERSHELL)
endif()

include(GNUInstallDirs)
install(TARGETS onedrive-gui)
configure_file(onedrive-gui.desktop.in onedrive-gui.desktop @ONLY)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/onedrive-gui.desktop DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
