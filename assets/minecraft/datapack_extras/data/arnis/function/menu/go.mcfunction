# Lands on the street beside the place; the top block there is used only if that fails.
$execute store success score #ok arnis run spreadplayers $(x) $(z) 0 1 under $(y) false @s
$execute if score #ok arnis matches 0 run spreadplayers $(x) $(z) 0 3 false @s
