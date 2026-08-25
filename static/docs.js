const toast = document.getElementById("toast");

document.querySelectorAll("pre").forEach((element) => {
  element.addEventListener("click", async () => {
    await navigator.clipboard.writeText(element.innerText);
    toast.classList.add("on");
    setTimeout(() => toast.classList.remove("on"), 700);
  });
});
