#include <cstdint>
#include <cstdio>
#include <vector>

#include "bridge_output_mapper.h"

namespace {

int failures = 0;

void Check(bool cond, const char* what) {
    if (!cond) {
        std::printf("FAIL: %s\n", what);
        ++failures;
    }
}

bool AllZero(const std::vector<float>& values) {
    for (const float v : values) {
        if (v != 0.0f) {
            return false;
        }
    }
    return true;
}

}  // namespace

int main() {
    using tdx_czsc::AnalysisSnapshotCpp;
    using tdx_czsc::FillFromSnapshot;
    using tdx_czsc::FillZeros;

    // FillZeros 覆盖全部输出。
    {
        std::vector<float> out(4, 9.0f);
        FillZeros(static_cast<int>(out.size()), out.data());
        Check(AllZero(out), "FillZeros zeroes every slot");
    }

    // 通道存在且等长：原样拷贝。
    {
        AnalysisSnapshotCpp snapshot;
        snapshot.channels[101] = {1.0f, 2.0f, 3.0f};
        std::vector<float> out(3, 0.0f);
        Check(FillFromSnapshot(snapshot, 101, 3, out.data()), "exact channel returns true");
        Check(out[0] == 1.0f && out[1] == 2.0f && out[2] == 3.0f, "exact channel copied");
    }

    // 通道短于 data_len：拷贝后尾部补零。
    {
        AnalysisSnapshotCpp snapshot;
        snapshot.channels[201] = {5.0f, 6.0f};
        std::vector<float> out(4, 7.0f);
        Check(FillFromSnapshot(snapshot, 201, 4, out.data()), "short channel returns true");
        Check(out[0] == 5.0f && out[1] == 6.0f, "short channel copies head");
        Check(out[2] == 0.0f && out[3] == 0.0f, "short channel zero-fills tail");
    }

    // 通道比 data_len 长：只取前 data_len 个。
    {
        AnalysisSnapshotCpp snapshot;
        snapshot.channels[301] = {1.0f, 2.0f, 3.0f, 4.0f};
        std::vector<float> out(2, 0.0f);
        Check(FillFromSnapshot(snapshot, 301, 2, out.data()), "long channel returns true");
        Check(out[0] == 1.0f && out[1] == 2.0f, "long channel truncated to data_len");
    }

    // 通道缺失：清零并返回 false。
    {
        AnalysisSnapshotCpp snapshot;
        snapshot.channels[101] = {1.0f};
        std::vector<float> out(3, 8.0f);
        Check(!FillFromSnapshot(snapshot, 999, 3, out.data()), "missing channel returns false");
        Check(AllZero(out), "missing channel zero-fills output");
    }

    // 非法入参：不清空输出，直接返回 false。
    {
        AnalysisSnapshotCpp snapshot;
        snapshot.channels[101] = {1.0f};
        std::vector<float> out(3, 4.0f);
        Check(!FillFromSnapshot(snapshot, 101, 0, out.data()), "zero data_len returns false");
        Check(!FillFromSnapshot(snapshot, 101, -1, out.data()), "negative data_len returns false");
        Check(!FillFromSnapshot(snapshot, 101, 3, nullptr), "null output returns false");
        Check(out[0] == 4.0f, "invalid call leaves output untouched");
    }

    if (failures != 0) {
        std::printf("output_mapper_test: %d failure(s)\n", failures);
        return 1;
    }
    std::printf("output_mapper_test: ok\n");
    return 0;
}
