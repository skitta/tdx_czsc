#include <cmath>
#include <cstdint>
#include <iostream>
#include <vector>

#include "bridge_output_mapper.h"

namespace {

bool Same(float a, float b) {
    return std::fabs(a - b) < 1e-6f;
}

}  // namespace

int main() {
    tdx_czsc::AnalysisSnapshotCpp snapshot;
    float out[4] = {1.0f, 1.0f, 1.0f, 1.0f};

    // 缺通道：返回 false，输出清零。
    if (tdx_czsc::FillFromSnapshot(snapshot, 305, 4, out)) {
        std::cerr << "missing channel should return false\n";
        return 1;
    }
    for (float value : out) {
        if (!Same(value, 0.0f)) {
            std::cerr << "missing channel should zero output\n";
            return 1;
        }
    }

    // 308 虚/实/无中枢编码原样回填。
    snapshot.channels[308] = {-11.0f, -10.0f, 12.0f, 0.0f};
    if (!tdx_czsc::FillFromSnapshot(snapshot, 308, 4, out)) {
        std::cerr << "308 channel should be found\n";
        return 1;
    }
    const float expected[4] = {-11.0f, -10.0f, 12.0f, 0.0f};
    for (int i = 0; i < 4; ++i) {
        if (!Same(out[i], expected[i])) {
            std::cerr << "308 value mismatch at " << i << '\n';
            return 1;
        }
    }

    // 407 段级三买价格回填。
    snapshot.channels[407] = {7.5f, 0.0f, 0.0f, 0.0f};
    if (!tdx_czsc::FillFromSnapshot(snapshot, 407, 4, out) || !Same(out[0], 7.5f)) {
        std::cerr << "407 value mismatch\n";
        return 1;
    }

    std::cout << "output_mapper_test ok\n";
    return 0;
}
