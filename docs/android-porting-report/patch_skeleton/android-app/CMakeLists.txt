cmake_minimum_required(VERSION 3.22.1)
project(video_hw_android_app)

# Option A: cargo-ndk builds libvideo_hw_android.so into src/main/jniLibs/<abi>/.
# In that case, no CMake build is required for Rust itself.

# Option B: import a prebuilt Rust shared library.
# add_library(video_hw_android SHARED IMPORTED)
# set_target_properties(video_hw_android PROPERTIES
#     IMPORTED_LOCATION ${CMAKE_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libvideo_hw_android.so)

# target_link_libraries(video_hw_android
#     mediandk
#     android
#     log
# )
