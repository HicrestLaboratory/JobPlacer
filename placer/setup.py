from setuptools import setup
from setuptools_rust import Binding, RustExtension

setup(
    name="job_placer",
    version="0.1.0",
    rust_extensions=[
        RustExtension(
            "job_placer",
            binding=Binding.PyO3,
            features=["python"]
        )
    ],
    zip_safe=False,
)