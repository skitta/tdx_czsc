cmake_minimum_required(VERSION 3.20)
set(CMAKE_TOOLCHAIN_FILE "E:/DELL/Documents/Codes/vcpkg/scripts/buildsystems/vcpkg.cmake" CACHE STRING "")
project(tdx_czsc_bridge LANGUAGES CXX)

if(NOT WIN32)
    message(WARNING "tdx_czsc_bridge is a Windows-only DLL target. Configure/build this target on Windows.")
    return()
endif()

find_package(Protobuf REQUIRED)

set(TDX_CZSC_PROTO_FILE ${CMAKE_CURRENT_SOURCE_DIR}/../proto/analysis.proto)
protobuf_generate_cpp(TDX_CZSC_PROTO_SRCS TDX_CZSC_PROTO_HDRS ${TDX_CZSC_PROTO_FILE})

add_library(tdx_czsc_bridge SHARED
    src/exports.cpp
    src/bridge_functions.cpp
    src/bridge_cache.cpp
    src/bridge_config.cpp
    src/bridge_ipc_client.cpp
    src/bridge_logging.cpp
    src/bridge_output_mapper.cpp
    ${TDX_CZSC_PROTO_SRCS}
    ${TDX_CZSC_PROTO_HDRS}
)

target_include_directories(tdx_czsc_bridge
    PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        ${CMAKE_CURRENT_BINARY_DIR}
)

target_compile_features(tdx_czsc_bridge PRIVATE cxx_std_20)

target_compile_definitions(tdx_czsc_bridge PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)

if(MSVC)
    target_compile_options(tdx_czsc_bridge PRIVATE /W4 /EHsc /utf-8)
else()
    target_compile_options(tdx_czsc_bridge PRIVATE -Wall -Wextra -Wpedantic)
endif()

if(TARGET protobuf::libprotobuf)
    target_link_libraries(tdx_czsc_bridge PRIVATE protobuf::libprotobuf)
else()
    target_include_directories(tdx_czsc_bridge PRIVATE ${Protobuf_INCLUDE_DIRS})
    target_link_libraries(tdx_czsc_bridge PRIVATE ${Protobuf_LIBRARIES})
endif()

set_target_properties(tdx_czsc_bridge PROPERTIES
    OUTPUT_NAME "tdx_czsc_bridge"
)

add_executable(tdx_czsc_ipc_smoke_test
    tests/ipc_client_smoke.cpp
    src/bridge_ipc_client.cpp
    src/bridge_config.cpp
    src/bridge_logging.cpp
    ${TDX_CZSC_PROTO_SRCS}
    ${TDX_CZSC_PROTO_HDRS}
)

# 宿主模拟测试：模拟通达信从不同目录加载 DLL，验证服务自动拉起与生命周期。
add_executable(tdx_czsc_deploy_smoke
    tests/deploy_host_smoke.cpp
)

target_include_directories(tdx_czsc_deploy_smoke
    PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/include
)

target_compile_features(tdx_czsc_deploy_smoke PRIVATE cxx_std_20)
target_compile_definitions(tdx_czsc_deploy_smoke PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)

if(MSVC)
    target_compile_options(tdx_czsc_deploy_smoke PRIVATE /W4 /EHsc /utf-8)
else()
    target_compile_options(tdx_czsc_deploy_smoke PRIVATE -Wall -Wextra -Wpedantic)
endif()

target_include_directories(tdx_czsc_ipc_smoke_test
    PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/include
        ${CMAKE_CURRENT_BINARY_DIR}
)

target_compile_features(tdx_czsc_ipc_smoke_test PRIVATE cxx_std_20)
target_compile_definitions(tdx_czsc_ipc_smoke_test PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)

if(MSVC)
    target_compile_options(tdx_czsc_ipc_smoke_test PRIVATE /W4 /EHsc /utf-8)
else()
    target_compile_options(tdx_czsc_ipc_smoke_test PRIVATE -Wall -Wextra -Wpedantic)
endif()

if(TARGET protobuf::libprotobuf)
    target_link_libraries(tdx_czsc_ipc_smoke_test PRIVATE protobuf::libprotobuf)
else()
    target_include_directories(tdx_czsc_ipc_smoke_test PRIVATE ${Protobuf_INCLUDE_DIRS})
    target_link_libraries(tdx_czsc_ipc_smoke_test PRIVATE ${Protobuf_LIBRARIES})
endif()
