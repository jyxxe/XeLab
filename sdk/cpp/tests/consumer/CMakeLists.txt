cmake_minimum_required(VERSION 3.22)
project(xelab_sdk_consumer LANGUAGES CXX)

add_subdirectory(../.. xelab-sdk)
add_executable(xelab-sdk-consumer main.cc task_worker.cc)
target_link_libraries(xelab-sdk-consumer PRIVATE xelab_plugin_sdk)
