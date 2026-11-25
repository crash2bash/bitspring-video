# RIFE, as the engine uses it

Frames in between by a neural network: RIFE (Real-Time Intermediate Flow Estimation), run
through ncnn on the GPU. `c/between_rife.cpp` is the engine's own code around it; what is
here comes from elsewhere:

- `warp.cpp`, `rife_ops.h`, `warp*.comp`: the model's one layer that ncnn does not have, from
  [rife-ncnn-vulkan](https://github.com/nihui/rife-ncnn-vulkan) at commit a7532fc. One thing
  is taken out of `warp.cpp`: the variant for values packed in eights, which ncnn has since
  dropped (`Option::use_shader_pack8` is gone). `warp*.comp.hex.h` are the shaders as bytes, made the way that project's cmake makes them
  (the comment above `#version` dropped, indentation stripped).
- `rife-v4.6/flownet.param` and `flownet.bin`: the model, version 4.6, from that project's
  release 20221029. It is built into the program.

Both under the MIT licence in `LICENSE` (nihui; the model after hzwer's
[RIFE](https://github.com/hzwer/ECCV2022-RIFE), also MIT).
